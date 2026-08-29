//! Domain models. SQL-backed via the manual `FromLibsqlRow` mappers below.
//!
//! Each struct maps from a `libsql::Row` BY COLUMN NAME (not positional index). This is
//! mandatory because some structs gain extra columns only in certain join queries (e.g.
//! `Account::consent_nickname` / `pending_net_cents` appear only in `list_all_enabled_accounts`),
//! and `User` is loaded via `SELECT *`. A positional mapper would misalign when an optional
//! column is absent. The name resolver below is tolerant of absent columns: a missing column
//! yields `None` rather than panicking.

use chrono::Utc;
use serde::Serialize;

/// Manual row → struct mapper for the libsql backend (replaces `sqlx::FromRow`).
pub trait FromLibsqlRow: Sized {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self>;
}

/// Resolves column-name → positional index for a libsql Row once, so mappers can fetch by name
/// and tolerate columns that are absent from a given SELECT.
///
/// Note: libsql's `FromValue` trait (the bound on `Row::get`) is sealed and not publicly
/// nameable, so this helper deliberately deals only in indices — the actual `row.get::<T>(idx)`
/// call lives at each field site (where the concrete `T` is monomorphised without naming the
/// trait). Mappers use the `col!` / `col_opt!` macros below to keep that terse.
pub struct ColumnIndex {
    names: Vec<Option<String>>,
}

impl ColumnIndex {
    pub fn new(row: &libsql::Row) -> Self {
        let n = row.column_count();
        let mut names = Vec::with_capacity(n as usize);
        for i in 0..n {
            names.push(row.column_name(i).map(|s| s.to_string()));
        }
        Self { names }
    }

    /// Index of a column by name, or None if the SELECT didn't include it.
    pub fn idx(&self, name: &str) -> Option<i32> {
        self.names
            .iter()
            .position(|c| c.as_deref() == Some(name))
            .map(|p| p as i32)
    }

    /// Index of a required column; errors if the SELECT omitted it.
    pub fn req(&self, name: &str) -> anyhow::Result<i32> {
        self.idx(name)
            .ok_or_else(|| anyhow::anyhow!("column '{name}' missing from row"))
    }
}

/// Fetch a required column by name and convert to the field's type.
/// `$c` is a `ColumnIndex`, `$row` the `libsql::Row`.
macro_rules! col {
    ($c:expr, $row:expr, $name:literal) => {{
        let i = $c.req($name)?;
        $row.get(i)?
    }};
}

/// Fetch a column that may be entirely ABSENT from the SELECT (=> None) or present-but-NULL
/// (=> None). Used for `#[sqlx(default)]`-style join-only columns; the field type must be
/// `Option<_>`.
macro_rules! col_opt {
    ($c:expr, $row:expr, $name:literal) => {{
        match $c.idx($name) {
            Some(i) => $row.get(i)?,
            None => None,
        }
    }};
}

#[derive(Debug, Clone, Serialize)]
pub struct Consent {
    pub id: i64,
    pub nickname: String,
    pub credentials_id: String,
    pub provider_id: Option<String>,
    pub provider_display_name: Option<String>,
    #[serde(skip)]
    pub access_token_enc: Vec<u8>,
    #[serde(skip)]
    pub access_token_nonce: Vec<u8>,
    #[serde(skip)]
    pub refresh_token_enc: Vec<u8>,
    #[serde(skip)]
    pub refresh_token_nonce: Vec<u8>,
    pub expires_at: i64,
    pub consent_expires_at: Option<i64>,
    pub scopes: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_sync_at: Option<i64>,
    pub last_sync_status: Option<String>,
    pub last_sync_error: Option<String>,
    pub enabled: i64,
}

impl FromLibsqlRow for Consent {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            nickname: col!(c, row, "nickname"),
            credentials_id: col!(c, row, "credentials_id"),
            provider_id: col!(c, row, "provider_id"),
            provider_display_name: col!(c, row, "provider_display_name"),
            access_token_enc: col!(c, row, "access_token_enc"),
            access_token_nonce: col!(c, row, "access_token_nonce"),
            refresh_token_enc: col!(c, row, "refresh_token_enc"),
            refresh_token_nonce: col!(c, row, "refresh_token_nonce"),
            expires_at: col!(c, row, "expires_at"),
            consent_expires_at: col!(c, row, "consent_expires_at"),
            scopes: col!(c, row, "scopes"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
            last_sync_at: col!(c, row, "last_sync_at"),
            last_sync_status: col!(c, row, "last_sync_status"),
            last_sync_error: col!(c, row, "last_sync_error"),
            enabled: col!(c, row, "enabled"),
        })
    }
}

impl Consent {
    pub fn is_consent_expired(&self) -> bool {
        match self.consent_expires_at {
            Some(deadline) => Utc::now().timestamp() >= deadline,
            None => false,
        }
    }

    pub fn is_access_token_expired(&self) -> bool {
        Utc::now().timestamp() >= (self.expires_at - 60)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Account {
    pub id: i64,
    pub consent_id: i64,
    pub truelayer_id: String,
    pub kind: String,
    pub display_name: String,
    pub iban: Option<String>,
    pub sort_code: Option<String>,
    pub account_number: Option<String>,
    pub card_last4: Option<String>,
    pub currency: String,
    pub firefly_account_id: Option<i64>,
    pub enabled: i64,
    pub created_at: i64,
    pub updated_at: i64,
    // 0006 — balance snapshot + metadata + user label override
    pub current_balance_cents: Option<i64>,
    pub available_balance_cents: Option<i64>,
    pub overdraft_cents: Option<i64>,
    pub credit_limit_cents: Option<i64>,
    pub last_statement_balance_cents: Option<i64>,
    pub last_statement_date: Option<String>,
    pub payment_due_cents: Option<i64>,
    pub payment_due_date: Option<String>,
    pub account_type: Option<String>,
    pub card_network: Option<String>,
    pub name_on_card: Option<String>,
    pub custom_display_name: Option<String>,
    pub balance_updated_at: Option<i64>,
    // Not a column on `accounts` — populated by joins that select `c.nickname AS consent_nickname`
    // (e.g. the accounts list) so the UI can label tiles with the user's chosen bank nickname.
    // Absent from every non-join SELECT → resolved via get_opt_absent (=> None when missing).
    pub consent_nickname: Option<String>,
    // Signed net of this account's PENDING transactions (credits +, debits −), so the UI can show
    // a "true spendable after pending" figure. Computed in list_all_enabled_accounts only.
    pub pending_net_cents: Option<i64>,
}

impl FromLibsqlRow for Account {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            consent_id: col!(c, row, "consent_id"),
            truelayer_id: col!(c, row, "truelayer_id"),
            kind: col!(c, row, "kind"),
            display_name: col!(c, row, "display_name"),
            iban: col!(c, row, "iban"),
            sort_code: col!(c, row, "sort_code"),
            account_number: col!(c, row, "account_number"),
            card_last4: col!(c, row, "card_last4"),
            currency: col!(c, row, "currency"),
            firefly_account_id: col!(c, row, "firefly_account_id"),
            enabled: col!(c, row, "enabled"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
            current_balance_cents: col!(c, row, "current_balance_cents"),
            available_balance_cents: col!(c, row, "available_balance_cents"),
            overdraft_cents: col!(c, row, "overdraft_cents"),
            credit_limit_cents: col!(c, row, "credit_limit_cents"),
            last_statement_balance_cents: col!(c, row, "last_statement_balance_cents"),
            last_statement_date: col!(c, row, "last_statement_date"),
            payment_due_cents: col!(c, row, "payment_due_cents"),
            payment_due_date: col!(c, row, "payment_due_date"),
            account_type: col!(c, row, "account_type"),
            card_network: col!(c, row, "card_network"),
            name_on_card: col!(c, row, "name_on_card"),
            custom_display_name: col!(c, row, "custom_display_name"),
            balance_updated_at: col!(c, row, "balance_updated_at"),
            // Join-only columns: absent from most SELECTs → None.
            consent_nickname: col_opt!(c, row, "consent_nickname"),
            pending_net_cents: col_opt!(c, row, "pending_net_cents"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Reminder {
    pub id: i64,
    pub title: String,
    pub notes: Option<String>,
    pub freq: String,
    pub every_n: i64,
    pub anchor_day: Option<i64>,
    pub due_at: i64,
    pub notify_before: i64,
    pub notify_enabled: i64,
    pub completed_at: Option<i64>,
    pub notified_at: Option<i64>,
    pub archived: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FromLibsqlRow for Reminder {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            title: col!(c, row, "title"),
            notes: col!(c, row, "notes"),
            freq: col!(c, row, "freq"),
            every_n: col!(c, row, "every_n"),
            anchor_day: col!(c, row, "anchor_day"),
            due_at: col!(c, row, "due_at"),
            notify_before: col!(c, row, "notify_before"),
            notify_enabled: col!(c, row, "notify_enabled"),
            completed_at: col!(c, row, "completed_at"),
            notified_at: col!(c, row, "notified_at"),
            archived: col!(c, row, "archived"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RecurringEntry {
    pub id: i64,
    pub account_id: i64,
    pub truelayer_id: String,
    pub kind: String,
    pub name: String,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub frequency: Option<String>,
    pub next_payment_date: Option<String>,
    pub status: Option<String>,
    pub firefly_bill_id: Option<i64>,
    pub last_seen_at: i64,
}

impl FromLibsqlRow for RecurringEntry {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            account_id: col!(c, row, "account_id"),
            truelayer_id: col!(c, row, "truelayer_id"),
            kind: col!(c, row, "kind"),
            name: col!(c, row, "name"),
            amount: col!(c, row, "amount"),
            currency: col!(c, row, "currency"),
            frequency: col!(c, row, "frequency"),
            next_payment_date: col!(c, row, "next_payment_date"),
            status: col!(c, row, "status"),
            firefly_bill_id: col!(c, row, "firefly_bill_id"),
            last_seen_at: col!(c, row, "last_seen_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncLogEntry {
    pub id: i64,
    pub consent_id: Option<i64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub status: String,
    pub accounts_synced: i64,
    pub transactions_imported: i64,
    pub transactions_skipped: i64,
    pub recurring_imported: i64,
    pub error_message: Option<String>,
}

impl FromLibsqlRow for SyncLogEntry {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            consent_id: col!(c, row, "consent_id"),
            started_at: col!(c, row, "started_at"),
            ended_at: col!(c, row, "ended_at"),
            status: col!(c, row, "status"),
            accounts_synced: col!(c, row, "accounts_synced"),
            transactions_imported: col!(c, row, "transactions_imported"),
            transactions_skipped: col!(c, row, "transactions_skipped"),
            recurring_imported: col!(c, row, "recurring_imported"),
            error_message: col!(c, row, "error_message"),
        })
    }
}

// ----- v2 models -----

#[derive(Debug, Clone, Serialize)]
pub struct Transaction {
    pub id: i64,
    pub account_id: i64,
    pub provider_txn_id: String,
    pub timestamp: i64,
    pub description: String,
    pub amount_cents: i64,
    pub currency: String,
    pub is_credit: i64,
    pub is_pending: i64,
    pub merchant_name: Option<String>,
    pub counterparty_iban: Option<String>,
    pub counterparty_name: Option<String>,
    pub category_id: Option<i64>,
    pub notes: Option<String>,
    pub raw_json: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FromLibsqlRow for Transaction {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            account_id: col!(c, row, "account_id"),
            provider_txn_id: col!(c, row, "provider_txn_id"),
            timestamp: col!(c, row, "timestamp"),
            description: col!(c, row, "description"),
            amount_cents: col!(c, row, "amount_cents"),
            currency: col!(c, row, "currency"),
            is_credit: col!(c, row, "is_credit"),
            is_pending: col!(c, row, "is_pending"),
            merchant_name: col!(c, row, "merchant_name"),
            counterparty_iban: col!(c, row, "counterparty_iban"),
            counterparty_name: col!(c, row, "counterparty_name"),
            category_id: col!(c, row, "category_id"),
            notes: col!(c, row, "notes"),
            raw_json: col!(c, row, "raw_json"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub icon: Option<String>,
    pub colour: Option<String>,
    pub created_at: i64,
}

impl FromLibsqlRow for Category {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            name: col!(c, row, "name"),
            parent_id: col!(c, row, "parent_id"),
            icon: col!(c, row, "icon"),
            colour: col!(c, row, "colour"),
            created_at: col!(c, row, "created_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
}

impl FromLibsqlRow for Tag {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            name: col!(c, row, "name"),
            created_at: col!(c, row, "created_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    pub id: i64,
    pub name: String,
    pub enabled: i64,
    pub priority: i64,
    pub match_description_regex: Option<String>,
    pub match_merchant_regex: Option<String>,
    pub match_min_amount_cents: Option<i64>,
    pub match_max_amount_cents: Option<i64>,
    pub match_account_id: Option<i64>,
    pub match_is_credit: Option<i64>,
    pub set_category_id: Option<i64>,
    pub add_tag_ids: Option<String>,
    pub set_notes: Option<String>,
    pub times_applied: i64,
    pub last_applied_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FromLibsqlRow for Rule {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            name: col!(c, row, "name"),
            enabled: col!(c, row, "enabled"),
            priority: col!(c, row, "priority"),
            match_description_regex: col!(c, row, "match_description_regex"),
            match_merchant_regex: col!(c, row, "match_merchant_regex"),
            match_min_amount_cents: col!(c, row, "match_min_amount_cents"),
            match_max_amount_cents: col!(c, row, "match_max_amount_cents"),
            match_account_id: col!(c, row, "match_account_id"),
            match_is_credit: col!(c, row, "match_is_credit"),
            set_category_id: col!(c, row, "set_category_id"),
            add_tag_ids: col!(c, row, "add_tag_ids"),
            set_notes: col!(c, row, "set_notes"),
            times_applied: col!(c, row, "times_applied"),
            last_applied_at: col!(c, row, "last_applied_at"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Budget {
    pub id: i64,
    pub name: String,
    pub category_id: Option<i64>,
    pub amount_cents: i64,
    pub period: String,
    pub currency: String,
    pub rollover: i64,
    pub enabled: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FromLibsqlRow for Budget {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            name: col!(c, row, "name"),
            category_id: col!(c, row, "category_id"),
            amount_cents: col!(c, row, "amount_cents"),
            period: col!(c, row, "period"),
            currency: col!(c, row, "currency"),
            rollover: col!(c, row, "rollover"),
            enabled: col!(c, row, "enabled"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Broker {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub currency: String,
    pub notes: Option<String>,
    pub enabled: i64,
    pub created_at: i64,
}

impl FromLibsqlRow for Broker {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            name: col!(c, row, "name"),
            kind: col!(c, row, "kind"),
            currency: col!(c, row, "currency"),
            notes: col!(c, row, "notes"),
            enabled: col!(c, row, "enabled"),
            created_at: col!(c, row, "created_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Holding {
    pub id: i64,
    pub broker_id: i64,
    pub symbol: String,
    pub asset_class: String,
    pub quantity: f64,
    pub avg_cost_per_unit: Option<f64>,
    pub currency: String,
    pub name: Option<String>,
    pub last_synced_at: Option<i64>,
    pub enabled: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FromLibsqlRow for Holding {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            broker_id: col!(c, row, "broker_id"),
            symbol: col!(c, row, "symbol"),
            asset_class: col!(c, row, "asset_class"),
            quantity: col!(c, row, "quantity"),
            avg_cost_per_unit: col!(c, row, "avg_cost_per_unit"),
            currency: col!(c, row, "currency"),
            name: col!(c, row, "name"),
            last_synced_at: col!(c, row, "last_synced_at"),
            enabled: col!(c, row, "enabled"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HoldingActivity {
    pub id: i64,
    pub holding_id: i64,
    pub activity_type: String,
    pub timestamp: i64,
    pub quantity: f64,
    pub price_per_unit: Option<f64>,
    pub fee: f64,
    pub currency: String,
    pub notes: Option<String>,
    pub created_at: i64,
}

impl FromLibsqlRow for HoldingActivity {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            holding_id: col!(c, row, "holding_id"),
            activity_type: col!(c, row, "activity_type"),
            timestamp: col!(c, row, "timestamp"),
            quantity: col!(c, row, "quantity"),
            price_per_unit: col!(c, row, "price_per_unit"),
            fee: col!(c, row, "fee"),
            currency: col!(c, row, "currency"),
            notes: col!(c, row, "notes"),
            created_at: col!(c, row, "created_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LatestQuote {
    pub symbol: String,
    pub price: f64,
    pub currency: String,
    pub fetched_at: i64,
    pub previous_close: Option<f64>,
    pub day_change_pct: Option<f64>,
    pub company_name: Option<String>,
}

impl FromLibsqlRow for LatestQuote {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            symbol: col!(c, row, "symbol"),
            price: col!(c, row, "price"),
            currency: col!(c, row, "currency"),
            fetched_at: col!(c, row, "fetched_at"),
            previous_close: col!(c, row, "previous_close"),
            day_change_pct: col!(c, row, "day_change_pct"),
            company_name: col!(c, row, "company_name"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Bill {
    pub id: i64,
    pub name: String,
    pub expected_amount_min_cents: i64,
    pub expected_amount_max_cents: i64,
    pub currency: String,
    pub repeat_freq: String,
    pub next_expected_date: Option<i64>,
    pub last_paid_date: Option<i64>,
    pub match_description_regex: Option<String>,
    pub enabled: i64,
    pub source_recurring_id: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FromLibsqlRow for Bill {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            name: col!(c, row, "name"),
            expected_amount_min_cents: col!(c, row, "expected_amount_min_cents"),
            expected_amount_max_cents: col!(c, row, "expected_amount_max_cents"),
            currency: col!(c, row, "currency"),
            repeat_freq: col!(c, row, "repeat_freq"),
            next_expected_date: col!(c, row, "next_expected_date"),
            last_paid_date: col!(c, row, "last_paid_date"),
            match_description_regex: col!(c, row, "match_description_regex"),
            enabled: col!(c, row, "enabled"),
            source_recurring_id: col!(c, row, "source_recurring_id"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
        })
    }
}

// ----- v3 — planning / "Ahead" layer -----

#[derive(Debug, Clone, Serialize)]
pub struct PlanAccount {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub source: String,
    pub linked_account_id: Option<i64>,
    pub overflow_account_id: Option<i64>,
    pub balance_cents: i64,
    pub currency: String,
    pub floor_cents: i64,
    pub cliff_date: Option<String>,
    pub cliff_new_floor_cents: Option<i64>,
    pub credit_limit_cents: Option<i64>,
    pub apr_bps: Option<i64>,
    pub statement_day: Option<i64>,
    pub payment_intent: Option<String>,
    pub balance_updated_at: Option<i64>,
    pub sort_order: i64,
    pub enabled: i64,
    pub created_at: i64,
    pub updated_at: i64,
    // Join-only: the live balance for synced accounts (read from `accounts`), else balance_cents.
    // Present only in list_plan_accounts; absent (=> None) elsewhere.
    pub resolved_balance_cents: Option<i64>,
}

impl FromLibsqlRow for PlanAccount {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            name: col!(c, row, "name"),
            kind: col!(c, row, "kind"),
            source: col!(c, row, "source"),
            linked_account_id: col!(c, row, "linked_account_id"),
            overflow_account_id: col!(c, row, "overflow_account_id"),
            balance_cents: col!(c, row, "balance_cents"),
            currency: col!(c, row, "currency"),
            floor_cents: col!(c, row, "floor_cents"),
            cliff_date: col!(c, row, "cliff_date"),
            cliff_new_floor_cents: col!(c, row, "cliff_new_floor_cents"),
            credit_limit_cents: col!(c, row, "credit_limit_cents"),
            apr_bps: col!(c, row, "apr_bps"),
            statement_day: col!(c, row, "statement_day"),
            payment_intent: col!(c, row, "payment_intent"),
            balance_updated_at: col!(c, row, "balance_updated_at"),
            sort_order: col!(c, row, "sort_order"),
            enabled: col!(c, row, "enabled"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
            resolved_balance_cents: col_opt!(c, row, "resolved_balance_cents"),
        })
    }
}

impl PlanAccount {
    /// The authoritative "now" balance: the live synced balance when present, else the stored one.
    pub fn balance(&self) -> i64 {
        self.resolved_balance_cents.unwrap_or(self.balance_cents)
    }
    /// Balance signed for the forecast. Synced credit cards report what you OWE as a POSITIVE
    /// number (TrueLayer convention); flip it negative so a card debt reads like one and the
    /// running balance / net total are correct. Manual cards are already stored negative.
    pub fn forecast_balance(&self) -> i64 {
        let b = self.balance();
        if self.source == "synced" && self.kind == "credit" {
            -b.abs()
        } else {
            b
        }
    }
    /// Active floor on a given ISO date, applying the cliff if it has passed.
    pub fn floor_on(&self, date_iso: &str) -> i64 {
        match (&self.cliff_date, self.cliff_new_floor_cents) {
            (Some(d), Some(nf)) if date_iso >= d.as_str() => nf,
            _ => self.floor_cents,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanEvent {
    pub id: i64,
    pub date: String,
    pub label: String,
    pub source: String,
    pub account_id: Option<i64>,
    pub to_account_id: Option<i64>,
    pub amount_cents: i64,
    pub recurrence: String,
    pub recur_until: Option<String>,
    pub category_id: Option<i64>,
    pub match_regex: Option<String>,
    pub matched_txn_id: Option<i64>,
    pub note: Option<String>,
    pub enabled: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FromLibsqlRow for PlanEvent {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            date: col!(c, row, "date"),
            label: col!(c, row, "label"),
            source: col!(c, row, "source"),
            account_id: col!(c, row, "account_id"),
            to_account_id: col!(c, row, "to_account_id"),
            amount_cents: col!(c, row, "amount_cents"),
            recurrence: col!(c, row, "recurrence"),
            recur_until: col!(c, row, "recur_until"),
            category_id: col!(c, row, "category_id"),
            match_regex: col!(c, row, "match_regex"),
            matched_txn_id: col!(c, row, "matched_txn_id"),
            note: col!(c, row, "note"),
            enabled: col!(c, row, "enabled"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Goal {
    pub id: i64,
    pub name: String,
    pub target_cents: i64,
    pub saved_cents: i64,
    pub source_account_id: Option<i64>,
    pub target_date: Option<String>,
    pub monthly_cents: i64,
    pub enabled: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl FromLibsqlRow for Goal {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: col!(c, row, "id"),
            name: col!(c, row, "name"),
            target_cents: col!(c, row, "target_cents"),
            saved_cents: col!(c, row, "saved_cents"),
            source_account_id: col!(c, row, "source_account_id"),
            target_date: col!(c, row, "target_date"),
            monthly_cents: col!(c, row, "monthly_cents"),
            enabled: col!(c, row, "enabled"),
            created_at: col!(c, row, "created_at"),
            updated_at: col!(c, row, "updated_at"),
        })
    }
}

// ----- TrueLayer DTOs -----

pub mod truelayer {
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    pub struct ApiResponse<T> {
        pub results: Vec<T>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct TokenResponse {
        pub access_token: String,
        pub refresh_token: String,
        pub expires_in: i64,
        #[serde(default)]
        pub scope: Option<String>,
        #[serde(default)]
        pub token_type: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct TLAccount {
        pub account_id: String,
        pub display_name: Option<String>,
        #[serde(default)]
        pub account_number: Option<AccountNumber>,
        #[serde(default)]
        pub currency: Option<String>,
        #[serde(default)]
        pub provider: Option<Provider>,
        #[serde(default)]
        pub account_type: Option<String>,
    }

    /// Response of GET /data/v1/accounts/{id}/balance.
    #[derive(Debug, Deserialize, Clone)]
    pub struct TLAccountBalance {
        pub currency: String,
        pub current: f64,
        #[serde(default)]
        pub available: Option<f64>,
        #[serde(default)]
        pub overdraft: Option<f64>,
        #[serde(default)]
        pub update_timestamp: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct AccountNumber {
        #[serde(default)]
        pub iban: Option<String>,
        #[serde(default)]
        pub sort_code: Option<String>,
        #[serde(default)]
        pub number: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct Provider {
        #[serde(default)]
        pub display_name: Option<String>,
        #[serde(default)]
        pub provider_id: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct TLCard {
        pub account_id: String,
        #[serde(default)]
        pub display_name: Option<String>,
        #[serde(default)]
        pub partial_card_number: Option<String>,
        #[serde(default)]
        pub currency: Option<String>,
        #[serde(default)]
        pub provider: Option<Provider>,
        #[serde(default)]
        pub card_network: Option<String>,
        #[serde(default)]
        pub name_on_card: Option<String>,
    }

    /// Response of GET /data/v1/cards/{id}/balance. Strictly richer than TLAccountBalance —
    /// credit-card fields included.
    #[derive(Debug, Deserialize, Clone)]
    pub struct TLCardBalance {
        pub currency: String,
        pub current: f64,
        #[serde(default)]
        pub available: Option<f64>,
        #[serde(default)]
        pub credit_limit: Option<f64>,
        #[serde(default)]
        pub last_statement_balance: Option<f64>,
        #[serde(default)]
        pub last_statement_date: Option<String>,
        #[serde(default)]
        pub payment_due: Option<f64>,
        #[serde(default)]
        pub payment_due_date: Option<String>,
        #[serde(default)]
        pub update_timestamp: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct TLTransaction {
        pub transaction_id: String,
        pub timestamp: String,
        pub description: String,
        pub amount: f64,
        pub currency: String,
        pub transaction_type: String,
        #[serde(default)]
        pub transaction_category: Option<String>,
        #[serde(default)]
        pub transaction_classification: Option<Vec<String>>,
        #[serde(default)]
        pub merchant_name: Option<String>,
        #[serde(default)]
        pub meta: Option<TLMeta>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct TLMeta {
        #[serde(default)]
        pub counter_party_iban: Option<String>,
        #[serde(default)]
        pub counter_party_preferred_name: Option<String>,
        #[serde(default)]
        pub provider_transaction_category: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct TLStandingOrder {
        #[serde(default)]
        pub standing_order_id: Option<String>,
        #[serde(default)]
        pub frequency: Option<String>,
        #[serde(default)]
        pub next_payment_date: Option<String>,
        // TrueLayer sends this as a bare number with currency carried separately — NOT a
        // {amount, currency} object. The old object shape made the whole response fail to parse
        // for any bank that actually populates it.
        #[serde(default)]
        pub next_payment_amount: Option<f64>,
        #[serde(default)]
        pub payee: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct TLDirectDebit {
        #[serde(default)]
        pub direct_debit_id: Option<String>,
        #[serde(default)]
        pub name: Option<String>,
        #[serde(default)]
        pub status: Option<String>,
        // Bare number (see note on TLStandingOrder), not a {amount, currency} object.
        #[serde(default)]
        pub previous_payment_amount: Option<f64>,
        // TrueLayer's field is `previous_payment_timestamp` (ISO-8601); accept the legacy
        // `previous_payment_date` spelling too.
        #[serde(default, alias = "previous_payment_date")]
        pub previous_payment_timestamp: Option<String>,
    }

    #[derive(Debug, Deserialize, Clone)]
    pub struct TLAmount {
        pub amount: f64,
        pub currency: String,
    }
}
