//! Optional notification hooks. Driven entirely by env vars — silently no-ops if unset.
//!
//! Env:
//! - `TALLY_HA_URL` + `TALLY_HA_TOKEN`: emit a Home Assistant REST event when a transaction lands.
//! - `TALLY_TELEGRAM_BOT_TOKEN` + `TALLY_TELEGRAM_CHAT_ID`: send Telegram message for transactions.
//! - `TALLY_TELEGRAM_MIN_AMOUNT`: only Telegram-notify transactions above this absolute amount (default: notify all).

use crate::models::truelayer::TLTransaction;
use crate::models::Account;
use reqwest::Client;
use serde_json::json;
use std::env;
use std::time::Duration;
use tracing::{debug, warn};

#[derive(Clone)]
pub struct Notifier {
    http: Client,
    pub ha_url: Option<String>,
    pub ha_token: Option<String>,
    pub telegram_token: Option<String>,
    pub telegram_chat: Option<String>,
    pub telegram_min_amount: f64,
}

impl Notifier {
    pub fn from_env() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(concat!("tally/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client build");

        let telegram_min_amount = env::var("TALLY_TELEGRAM_MIN_AMOUNT")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        Self {
            http,
            ha_url: env::var("TALLY_HA_URL").ok().filter(|s| !s.is_empty()),
            ha_token: env::var("TALLY_HA_TOKEN").ok().filter(|s| !s.is_empty()),
            telegram_token: env::var("TALLY_TELEGRAM_BOT_TOKEN")
                .ok()
                .filter(|s| !s.is_empty()),
            telegram_chat: env::var("TALLY_TELEGRAM_CHAT_ID")
                .ok()
                .filter(|s| !s.is_empty()),
            telegram_min_amount,
        }
    }

    pub async fn notify_transaction(&self, txn: &TLTransaction, account: &Account, is_pending: bool) {
        self.ha_event(txn, account, is_pending).await;
        if txn.amount.abs() >= self.telegram_min_amount {
            self.telegram_message(txn, account, is_pending).await;
        }
    }

    async fn ha_event(&self, txn: &TLTransaction, account: &Account, is_pending: bool) {
        let (Some(url), Some(token)) = (&self.ha_url, &self.ha_token) else {
            return;
        };
        let event_url = format!(
            "{}/api/events/tally_transaction",
            url.trim_end_matches('/')
        );
        let payload = json!({
            "account": account.display_name,
            "account_kind": account.kind,
            "amount": txn.amount,
            "currency": txn.currency,
            "description": txn.description,
            "timestamp": txn.timestamp,
            "transaction_type": txn.transaction_type,
            "merchant_name": txn.merchant_name,
            "is_pending": is_pending,
            "transaction_id": txn.transaction_id,
        });
        match self
            .http
            .post(&event_url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => {
                debug!("HA event emitted for txn {}", txn.transaction_id);
            }
            Ok(r) => warn!(
                "HA event failed: {} body: {}",
                r.status(),
                r.text().await.unwrap_or_default()
            ),
            Err(e) => warn!("HA event error: {e:#}"),
        }
    }

    async fn telegram_message(&self, txn: &TLTransaction, account: &Account, is_pending: bool) {
        let prefix = if is_pending { "⏳ pending" } else { "✅" };
        let sign = if txn.transaction_type.eq_ignore_ascii_case("DEBIT") {
            "-"
        } else {
            "+"
        };
        let text = format!(
            "{prefix} {sign}£{:.2} {} · {}\n_{}_",
            txn.amount.abs(),
            account.display_name,
            txn.description,
            txn.merchant_name.as_deref().unwrap_or(""),
        );
        self.send_telegram_text(&text, is_pending).await;
    }

    /// Send an arbitrary Markdown message via Telegram. No-op if Telegram isn't configured.
    /// Used by reminders and deal alerts as well as transaction notifications.
    pub async fn send_telegram_text(&self, text: &str, silent: bool) {
        let (Some(tok), Some(chat)) = (&self.telegram_token, &self.telegram_chat) else {
            return;
        };
        let url = format!("https://api.telegram.org/bot{tok}/sendMessage");
        let body = json!({
            "chat_id": chat,
            "text": text,
            "parse_mode": "Markdown",
            "disable_notification": silent,
        });
        match self.http.post(&url).json(&body).send().await {
            Ok(r) if r.status().is_success() => debug!("telegram sent"),
            Ok(r) => warn!("telegram failed: {} {}", r.status(), r.text().await.unwrap_or_default()),
            Err(e) => warn!("telegram error: {e:#}"),
        }
    }
}
