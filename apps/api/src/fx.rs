//! FX conversion to GBP for investment values.
//!
//! Rates come from Yahoo's synthetic FX tickers (`USDGBP=X` = pounds per 1 USD) and are
//! cached in `latest_quotes` alongside instrument quotes, so the SQL views and every
//! renderer can join/look them up without a network call. `GBp` pence quotes are already
//! normalised to pounds inside the Yahoo client and never reach this layer.

use crate::models::LatestQuote;
use crate::AppState;
use std::collections::{HashMap, HashSet};

/// Refetch an FX rate if the cached one is older than this. Rates move slowly relative
/// to how tally uses them; an hour keeps us honest without hammering Yahoo.
const FX_TTL_SECS: i64 = 3_600;

/// Yahoo ticker for converting 1 unit of `ccy` into GBP.
pub fn fx_symbol(ccy: &str) -> String {
    format!("{}GBP=X", ccy.to_uppercase())
}

/// Build a currency → GBP-rate map out of the FX rows cached in `latest_quotes`.
/// Always contains GBP → 1.0.
pub fn rates_from_quotes(quotes: &[LatestQuote]) -> HashMap<String, f64> {
    let mut rates = HashMap::new();
    rates.insert("GBP".to_string(), 1.0);
    for q in quotes {
        if let Some(ccy) = q.symbol.strip_suffix("GBP=X") {
            if ccy.len() == 3 && q.price > 0.0 {
                rates.insert(ccy.to_string(), q.price);
            }
        }
    }
    rates
}

/// Convert an amount in `ccy` to GBP. Falls back to 1:1 when no rate is cached yet —
/// wrong for USD, but strictly no worse than the pre-FX behaviour, and the next quote
/// refresh fixes it.
pub fn to_gbp(rates: &HashMap<String, f64>, amount: f64, ccy: &str) -> f64 {
    amount * rates.get(ccy).copied().unwrap_or(1.0)
}

/// Make sure `latest_quotes` holds a fresh GBP rate for every non-GBP currency given.
/// Best-effort: network errors are logged and swallowed, mirroring quote refresh.
pub async fn ensure_fx_quotes<I>(state: &AppState, currencies: I)
where
    I: IntoIterator<Item = String>,
{
    let needed: HashSet<String> = currencies
        .into_iter()
        .map(|c| c.to_uppercase())
        .filter(|c| c != "GBP" && c.len() == 3)
        .collect();
    if needed.is_empty() {
        return;
    }
    let now = chrono::Utc::now().timestamp();
    let existing = match state.db.all_latest_quotes().await {
        Ok(q) => q,
        Err(_) => return,
    };
    let fetched_by: HashMap<_, _> = existing
        .iter()
        .map(|q| (q.symbol.clone(), q.fetched_at))
        .collect();
    let to_fetch: Vec<String> = needed
        .into_iter()
        .map(|c| fx_symbol(&c))
        .filter(|sym| match fetched_by.get(sym) {
            Some(t) => now - t > FX_TTL_SECS,
            None => true,
        })
        .collect();
    if to_fetch.is_empty() {
        return;
    }
    let yc = crate::clients::yahoo::YahooClient::new();
    let refs: Vec<&str> = to_fetch.iter().map(|s| s.as_str()).collect();
    match yc.quotes(&refs).await {
        Ok(qs) => {
            for q in qs {
                let _ = state
                    .db
                    .upsert_latest_quote(
                        &q.symbol,
                        q.price,
                        &q.currency,
                        q.previous_close,
                        q.day_change_pct,
                        q.name.as_deref(),
                    )
                    .await;
            }
        }
        Err(e) => tracing::warn!("fx quote refresh: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(symbol: &str, price: f64) -> LatestQuote {
        LatestQuote {
            symbol: symbol.to_string(),
            price,
            currency: "GBP".to_string(),
            fetched_at: 0,
            previous_close: None,
            day_change_pct: None,
            company_name: None,
        }
    }

    #[test]
    fn rates_pick_up_fx_rows_only() {
        let quotes = vec![quote("INTC", 95.04), quote("USDGBP=X", 0.743)];
        let rates = rates_from_quotes(&quotes);
        assert_eq!(rates.get("USD"), Some(&0.743));
        assert_eq!(rates.get("GBP"), Some(&1.0));
        assert_eq!(rates.len(), 2);
    }

    #[test]
    fn to_gbp_converts_and_falls_back() {
        let rates = rates_from_quotes(&[quote("USDGBP=X", 0.75)]);
        assert!((to_gbp(&rates, 100.0, "USD") - 75.0).abs() < 1e-9);
        assert!((to_gbp(&rates, 50.0, "GBP") - 50.0).abs() < 1e-9);
        // Unknown currency: fall back to 1:1 rather than dropping the value.
        assert!((to_gbp(&rates, 20.0, "EUR") - 20.0).abs() < 1e-9);
    }
}
