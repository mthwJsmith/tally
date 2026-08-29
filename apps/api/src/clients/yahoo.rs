//! Yahoo Finance unofficial quote endpoint.
//!
//! Endpoint: https://query1.finance.yahoo.com/v7/finance/quote?symbols=AAPL,VWRP.L,VOD.L
//! Returns JSON with current price, name, previous close, day change %.
//! No auth required. Soft rate-limited; we batch up to ~50 symbols per call and cache.
//!
//! Yahoo's ToS allows personal use. For a commercial offering you'd use a paid provider
//! (Finnhub/Tiingo/Polygon). For self-host this is what Wealthfolio + many FOSS tools use.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

const QUOTE_URL: &str = "https://query1.finance.yahoo.com/v7/finance/quote";

#[derive(Clone)]
pub struct YahooClient {
    http: Client,
}

/// Yahoo quotes LSE-listed instruments (VUAG.L, most 0P… funds) in pence sterling,
/// reported as currency "GBp" (sometimes "GBX"). Normalise to pounds at the edge so
/// everything downstream deals in major units only.
fn normalize_ccy(price: f64, currency: &str) -> (f64, String) {
    match currency {
        "GBp" | "GBX" | "gbp_pence" => (price / 100.0, "GBP".to_string()),
        other => (price, other.to_string()),
    }
}

#[derive(Debug, Clone)]
pub struct Quote {
    pub symbol: String,
    pub price: f64,
    pub currency: String,
    pub name: Option<String>,
    pub previous_close: Option<f64>,
    pub day_change_pct: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct QuoteResponse {
    #[serde(rename = "quoteResponse")]
    quote_response: InnerResponse,
}

#[derive(Debug, Deserialize)]
struct InnerResponse {
    result: Vec<RawQuote>,
}

#[derive(Debug, Deserialize)]
struct RawQuote {
    symbol: String,
    #[serde(rename = "regularMarketPrice")]
    regular_market_price: Option<f64>,
    #[serde(rename = "regularMarketPreviousClose")]
    regular_market_previous_close: Option<f64>,
    #[serde(rename = "regularMarketChangePercent")]
    regular_market_change_percent: Option<f64>,
    #[serde(rename = "shortName")]
    short_name: Option<String>,
    #[serde(rename = "longName")]
    long_name: Option<String>,
    currency: Option<String>,
}

impl YahooClient {
    pub fn new() -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(15))
                // Yahoo's web API blocks clients without a browser UA.
                .user_agent(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36",
                )
                .build()
                .expect("reqwest client build"),
        }
    }

    /// Fetch quotes for a list of symbols.
    ///
    /// As of 2026, Yahoo's v7 `finance/quote` endpoint requires a session cookie + crumb
    /// (returns `Unauthorized` otherwise). v8 `finance/chart` still works without auth, so
    /// we hit that once per symbol and pull the spot price + previous close from `meta`.
    pub async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        if symbols.is_empty() {
            return Ok(vec![]);
        }
        let mut out = Vec::with_capacity(symbols.len());
        for sym in symbols {
            match self.quote_via_chart(sym).await {
                Ok(Some(q)) => out.push(q),
                Ok(None) => tracing::warn!("yahoo chart: no data for {sym}"),
                Err(e) => tracing::warn!("yahoo chart {sym}: {e:#}"),
            }
        }
        Ok(out)
    }

    async fn quote_via_chart(&self, symbol: &str) -> Result<Option<Quote>> {
        let resp = self
            .http
            .get(format!(
                "https://query1.finance.yahoo.com/v8/finance/chart/{symbol}"
            ))
            .query(&[("range", "1d"), ("interval", "1m")])
            .header("Accept", "application/json")
            .send()
            .await
            .with_context(|| format!("GET chart {symbol}"))?;
        if !resp.status().is_success() {
            return Err(anyhow!("yahoo chart {symbol}: {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await.context("decode chart")?;
        let meta = v
            .pointer("/chart/result/0/meta")
            .ok_or_else(|| anyhow!("no meta"))?;
        let price = meta
            .get("regularMarketPrice")
            .and_then(|x| x.as_f64());
        let Some(price) = price else { return Ok(None) };
        let raw_ccy = meta
            .get("currency")
            .and_then(|x| x.as_str())
            .unwrap_or("USD");
        let (price, currency) = normalize_ccy(price, raw_ccy);
        let prev = meta
            .get("chartPreviousClose")
            .or_else(|| meta.get("previousClose"))
            .and_then(|x| x.as_f64())
            .map(|p| normalize_ccy(p, raw_ccy).0);
        let day_pct = match prev {
            Some(p) if p > 0.0 => Some(((price - p) / p) * 100.0),
            _ => None,
        };
        Ok(Some(Quote {
            symbol: symbol.to_string(),
            price,
            currency,
            name: meta
                .get("longName")
                .or_else(|| meta.get("shortName"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            previous_close: prev,
            day_change_pct: day_pct,
        }))
    }

    pub async fn quote(&self, symbol: &str) -> Result<Option<Quote>> {
        Ok(self.quotes(&[symbol]).await?.into_iter().next())
    }

    /// Historical price series for a symbol.
    ///
    /// `range` accepts Yahoo's tokens: `1d`, `5d`, `1mo`, `3mo`, `6mo`, `1y`, `2y`, `5y`, `10y`, `ytd`, `max`.
    /// `interval` typically `1d` for ranges ≥ 1mo, `1h` or `30m` for shorter ranges.
    pub async fn history(
        &self,
        symbol: &str,
        range: &str,
        interval: &str,
    ) -> Result<Vec<HistoryPoint>> {
        // reqwest URL-encodes the symbol when we put it in a path segment via Url.
        let base = format!("https://query1.finance.yahoo.com/v8/finance/chart/{symbol}");
        let resp = self
            .http
            .get(&base)
            .query(&[("range", range), ("interval", interval)])
            .header("Accept", "application/json")
            .send()
            .await
            .with_context(|| format!("GET chart {symbol}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Yahoo chart {symbol}: {status} body: {body}"));
        }
        let parsed: ChartResponse = resp.json().await.context("decode Yahoo chart JSON")?;
        let Some(result) = parsed.chart.result.into_iter().next() else {
            return Ok(vec![]);
        };
        // Pence-quoted series (GBp) are normalised to pounds, same as spot quotes.
        let raw_ccy = result
            .meta
            .as_ref()
            .and_then(|m| m.currency.as_deref())
            .unwrap_or("USD");
        let timestamps = result.timestamp.unwrap_or_default();
        let closes = result
            .indicators
            .quote
            .into_iter()
            .next()
            .map(|q| q.close)
            .unwrap_or_default();
        let mut out = Vec::with_capacity(timestamps.len());
        for (i, ts) in timestamps.into_iter().enumerate() {
            if let Some(Some(c)) = closes.get(i) {
                out.push(HistoryPoint {
                    timestamp: ts,
                    close: normalize_ccy(*c, raw_ccy).0,
                });
            }
        }
        Ok(out)
    }

    /// Symbol/name autocomplete search. Backs the SymbolCombobox in the frontend.
    pub async fn search(&self, q: &str) -> Result<Vec<SymbolHit>> {
        let resp = self
            .http
            .get("https://query1.finance.yahoo.com/v1/finance/search")
            .query(&[("q", q), ("lang", "en-GB"), ("region", "GB"), ("quotesCount", "10")])
            .header("Accept", "application/json")
            .send()
            .await
            .context("GET yahoo search")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("yahoo search: {status} body: {body}"));
        }
        let parsed: SearchResponse = resp.json().await.context("decode yahoo search")?;
        Ok(parsed
            .quotes
            .into_iter()
            .filter_map(|q| {
                let symbol = q.symbol?;
                let name = q.longname.or(q.shortname).unwrap_or_else(|| symbol.clone());
                Some(SymbolHit {
                    symbol,
                    name,
                    exchange: q.exchange.unwrap_or_default(),
                    quote_type: q.quote_type.unwrap_or_default(),
                })
            })
            .collect())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HistoryPoint {
    pub timestamp: i64,
    pub close: f64,
}

#[derive(Debug, Deserialize)]
struct ChartResponse {
    chart: ChartInner,
}

#[derive(Debug, Deserialize)]
struct ChartInner {
    #[serde(default)]
    result: Vec<ChartResult>,
}

#[derive(Debug, Deserialize)]
struct ChartResult {
    #[serde(default)]
    meta: Option<ChartMeta>,
    #[serde(default)]
    timestamp: Option<Vec<i64>>,
    indicators: ChartIndicators,
}

#[derive(Debug, Deserialize)]
struct ChartMeta {
    #[serde(default)]
    currency: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChartIndicators {
    #[serde(default)]
    quote: Vec<ChartQuoteSeries>,
}

#[derive(Debug, Deserialize)]
struct ChartQuoteSeries {
    #[serde(default)]
    close: Vec<Option<f64>>,
}

#[derive(Debug, serde::Serialize)]
pub struct SymbolHit {
    pub symbol: String,
    pub name: String,
    pub exchange: String,
    pub quote_type: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    quotes: Vec<SearchHit>,
}

#[cfg(test)]
mod tests {
    use super::normalize_ccy;

    #[test]
    fn pence_quotes_become_pounds() {
        assert_eq!(normalize_ccy(10732.0, "GBp"), (107.32, "GBP".to_string()));
        assert_eq!(normalize_ccy(1530.0, "GBX"), (15.30, "GBP".to_string()));
        assert_eq!(normalize_ccy(95.04, "USD"), (95.04, "USD".to_string()));
        assert_eq!(normalize_ccy(107.32, "GBP"), (107.32, "GBP".to_string()));
    }
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    symbol: Option<String>,
    #[serde(default)]
    shortname: Option<String>,
    #[serde(default)]
    longname: Option<String>,
    #[serde(default)]
    exchange: Option<String>,
    #[serde(rename = "quoteType", default)]
    quote_type: Option<String>,
}
