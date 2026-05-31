//! Domain models. SQL-backed via sqlx::FromRow.

use chrono::Utc;
use serde::Serialize;
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize)]
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

#[derive(Debug, Clone, FromRow, Serialize)]
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
    #[sqlx(default)]
    pub consent_nickname: Option<String>,
    // Signed net of this account's PENDING transactions (credits +, debits −), so the UI can show
    // a "true spendable after pending" figure. Computed in list_all_enabled_accounts only.
    #[sqlx(default)]
    pub pending_net_cents: Option<i64>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
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

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct WatchlistItem {
    pub id: i64,
    pub name: String,
    pub keywords: Option<String>,
    pub target_price_cents: Option<i64>,
    pub currency: String,
    pub archived: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct WatchlistSource {
    pub id: i64,
    pub item_id: i64,
    pub kind: String,
    #[sqlx(rename = "ref")]
    #[serde(rename = "ref")]
    pub ref_: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DealObservation {
    pub id: i64,
    pub item_id: i64,
    pub title: String,
    pub url: Option<String>,
    pub price_cents: Option<i64>,
    pub source_kind: String,
    pub guid: String,
    pub found_at: i64,
    pub notified: i64,
}

#[derive(Debug, Clone, FromRow, Serialize)]
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

#[derive(Debug, Clone, FromRow, Serialize)]
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

// ----- v2 models -----

#[derive(Debug, Clone, FromRow, Serialize)]
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

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Category {
    pub id: i64,
    pub name: String,
    pub parent_id: Option<i64>,
    pub icon: Option<String>,
    pub colour: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Tag {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow, Serialize)]
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

#[derive(Debug, Clone, FromRow, Serialize)]
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

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Broker {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub currency: String,
    pub notes: Option<String>,
    pub enabled: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, FromRow, Serialize)]
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

#[derive(Debug, Clone, FromRow, Serialize)]
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

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct LatestQuote {
    pub symbol: String,
    pub price: f64,
    pub currency: String,
    pub fetched_at: i64,
    pub previous_close: Option<f64>,
    pub day_change_pct: Option<f64>,
    pub company_name: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
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
