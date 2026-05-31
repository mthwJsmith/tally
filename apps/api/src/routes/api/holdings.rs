//! Investments: brokers, holdings, activities, price quotes.

use crate::clients::yahoo::YahooClient;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}

const QUOTE_TTL_SECS: i64 = 300; // 5 min cache

/// Background-friendly quote refresh used by `list_holdings`. Fetches any symbol whose
/// `latest_quotes.fetched_at` is missing or older than QUOTE_TTL_SECS. Errors are logged
/// and swallowed — we never block the holdings list on Yahoo flakiness.
async fn refresh_stale_quotes(
    state: &Arc<AppState>,
    holdings: &[crate::models::Holding],
) {
    let now = chrono::Utc::now().timestamp();
    let existing = match state.db.all_latest_quotes().await {
        Ok(q) => q,
        Err(_) => return,
    };
    let fetched_by: std::collections::HashMap<_, _> = existing
        .iter()
        .map(|q| (q.symbol.clone(), q.fetched_at))
        .collect();
    let mut to_fetch: Vec<&str> = Vec::new();
    for h in holdings {
        match fetched_by.get(&h.symbol) {
            None => to_fetch.push(&h.symbol),
            Some(t) if now - t > QUOTE_TTL_SECS => to_fetch.push(&h.symbol),
            _ => {}
        }
    }
    if to_fetch.is_empty() {
        return;
    }
    let yc = YahooClient::new();
    match yc.quotes(&to_fetch).await {
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
        Err(e) => tracing::warn!("auto quote refresh: {e:#}"),
    }
}

// ---------------- brokers ----------------

#[derive(Debug, Deserialize)]
pub struct BrokerBody {
    pub name: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub notes: Option<String>,
}
fn default_kind() -> String {
    "general".to_string()
}
fn default_currency() -> String {
    "GBP".to_string()
}

pub async fn list_brokers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let brokers = state.db.list_brokers().await.map_err(internal)?;
    Ok(Json(json!({ "brokers": brokers })))
}

pub async fn create_broker(
    State(state): State<Arc<AppState>>,
    Json(b): Json<BrokerBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state
        .db
        .upsert_broker(&b.name, &b.kind, &b.currency, b.notes.as_deref())
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_broker(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_broker(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- holdings ----------------

#[derive(Debug, Deserialize)]
pub struct HoldingBody {
    pub broker_id: i64,
    pub symbol: String,
    #[serde(default = "default_asset_class")]
    pub asset_class: String,
    pub quantity: f64,
    pub avg_cost_per_unit: Option<f64>,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub name: Option<String>,
}
fn default_asset_class() -> String {
    "equity".to_string()
}

pub async fn list_holdings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let holdings = state.db.list_holdings().await.map_err(internal)?;
    // Lazy quote refresh — any symbol missing a quote or older than ~5 min gets fetched
    // now. Keeps the UI live without the user clicking "Sync prices".
    refresh_stale_quotes(&state, &holdings).await;
    let quotes = state.db.all_latest_quotes().await.map_err(internal)?;
    let by_symbol: std::collections::HashMap<_, _> =
        quotes.into_iter().map(|q| (q.symbol.clone(), q)).collect();

    // Compose per-holding view with live values + P&L.
    let enriched: Vec<_> = holdings
        .into_iter()
        .map(|h| {
            let q = by_symbol.get(&h.symbol);
            let price = q.map(|q| q.price);
            let market_value = price.map(|p| p * h.quantity);
            let cost_basis = h
                .avg_cost_per_unit
                .map(|c| c * h.quantity);
            let gain = match (market_value, cost_basis) {
                (Some(m), Some(c)) => Some(m - c),
                _ => None,
            };
            let gain_pct = match (gain, cost_basis) {
                (Some(g), Some(c)) if c.abs() > 1e-9 => Some((g / c) * 100.0),
                _ => None,
            };
            json!({
                "holding": h,
                "current_price": price,
                "market_value": market_value,
                "cost_basis": cost_basis,
                "gain": gain,
                "gain_pct": gain_pct,
                "day_change_pct": q.and_then(|q| q.day_change_pct),
                "company_name": q.and_then(|q| q.company_name.clone()),
            })
        })
        .collect();

    Ok(Json(json!({ "holdings": enriched })))
}

pub async fn create_holding(
    State(state): State<Arc<AppState>>,
    Json(b): Json<HoldingBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state
        .db
        .upsert_holding(
            b.broker_id,
            b.symbol.trim(),
            &b.asset_class,
            b.quantity,
            b.avg_cost_per_unit,
            &b.currency,
            b.name.as_deref(),
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete_holding(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_holding(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------- activities ----------------

/// One unified activity payload. Either `holding_id` is provided (existing position) or
/// `broker_id` + `symbol` are (Ghostfolio-style: first BUY for a new symbol auto-creates
/// the holding). After insert, the holding's derived quantity + avg cost are recomputed
/// from the full activity log — so activities are the source of truth.
#[derive(Debug, Deserialize)]
pub struct ActivityBody {
    pub holding_id: Option<i64>,
    pub broker_id: Option<i64>,
    pub symbol: Option<String>,
    pub asset_class: Option<String>,
    pub name: Option<String>,
    pub activity_type: String,
    pub timestamp: i64,
    pub quantity: f64,
    pub price_per_unit: Option<f64>,
    #[serde(default)]
    pub fee: f64,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub notes: Option<String>,
}

pub async fn create_activity(
    State(state): State<Arc<AppState>>,
    Json(b): Json<ActivityBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // Resolve the target holding. If the caller passed broker_id+symbol instead of an
    // existing holding_id, upsert the holding (initial qty 0, no avg cost — both come
    // from the recompute below).
    let holding_id = match b.holding_id {
        Some(id) => id,
        None => {
            let broker_id = b
                .broker_id
                .ok_or((StatusCode::BAD_REQUEST, "broker_id required".into()))?;
            let symbol = b
                .symbol
                .as_deref()
                .ok_or((StatusCode::BAD_REQUEST, "symbol required".into()))?;
            let asset_class = b.asset_class.as_deref().unwrap_or("equity");
            state
                .db
                .upsert_holding(
                    broker_id,
                    symbol.trim(),
                    asset_class,
                    0.0,
                    None,
                    &b.currency,
                    b.name.as_deref(),
                )
                .await
                .map_err(internal)?
        }
    };
    let id = state
        .db
        .create_activity(
            holding_id,
            &b.activity_type,
            b.timestamp,
            b.quantity,
            b.price_per_unit,
            b.fee,
            &b.currency,
            b.notes.as_deref(),
        )
        .await
        .map_err(internal)?;
    state
        .db
        .recompute_holding_from_activities(holding_id)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id, "holding_id": holding_id })))
}

pub async fn list_activities(
    State(state): State<Arc<AppState>>,
    Path(holding_id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let activities = state.db.list_activities(holding_id).await.map_err(internal)?;
    Ok(Json(json!({ "activities": activities })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateActivityBody {
    pub activity_type: String,
    pub timestamp: i64,
    pub quantity: f64,
    pub price_per_unit: Option<f64>,
    #[serde(default)]
    pub fee: f64,
    pub notes: Option<String>,
}

pub async fn update_activity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<UpdateActivityBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let holding_id = state
        .db
        .update_activity(
            id,
            &b.activity_type,
            b.timestamp,
            b.quantity,
            b.price_per_unit,
            b.fee,
            b.notes.as_deref(),
        )
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "activity not found".to_string()))?;
    state
        .db
        .recompute_holding_from_activities(holding_id)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true, "holding_id": holding_id })))
}

pub async fn delete_activity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // Capture the holding_id before delete so we can recompute its derived totals.
    let holding_id: Option<i64> = sqlx::query_scalar(
        "SELECT holding_id FROM holding_activities WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(internal)?;
    state.db.delete_activity(id).await.map_err(internal)?;
    if let Some(hid) = holding_id {
        state
            .db
            .recompute_holding_from_activities(hid)
            .await
            .map_err(internal)?;
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------- price sync ----------------

// ---------------- symbol autocomplete ----------------

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
}

pub async fn symbol_search(
    axum::extract::Query(q): axum::extract::Query<SearchQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let trimmed = q.q.trim();
    if trimmed.len() < 1 {
        return Ok(Json(json!({ "hits": [] })));
    }
    let yc = YahooClient::new();
    let hits = yc
        .search(trimmed)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("yahoo: {e:#}")))?;
    Ok(Json(json!({ "hits": hits })))
}

#[derive(Debug, Deserialize)]
pub struct SymbolHistoryQuery {
    pub symbol: String,
    #[serde(default = "default_range")]
    pub range: String,
    #[serde(default = "default_interval")]
    pub interval: String,
}

#[derive(Debug, Deserialize)]
pub struct PortfolioHistoryQuery {
    #[serde(default = "default_range")]
    pub range: String,
    #[serde(default = "default_interval")]
    pub interval: String,
    /// When true, ignore `range` and start at the earliest BUY/TRANSFER_IN timestamp
    /// across all holdings — i.e. "performance since I started investing".
    #[serde(default)]
    pub since_buy: bool,
}

fn default_range() -> String {
    "1mo".to_string()
}
fn default_interval() -> String {
    "1d".to_string()
}

/// GET /api/holdings/history?symbol=AAPL&range=1mo&interval=1d
/// Returns Yahoo close-price history for one symbol.
pub async fn symbol_history(
    axum::extract::Query(q): axum::extract::Query<SymbolHistoryQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let yc = YahooClient::new();
    let points = yc
        .history(q.symbol.trim(), &q.range, &q.interval)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("yahoo: {e:#}")))?;
    Ok(Json(json!({ "symbol": q.symbol, "points": points })))
}

/// GET /api/holdings/portfolio-history?range=1mo&interval=1d
/// Aggregates all holdings into one time series in their native currencies (no FX yet —
/// summed numerically, which is fine if all holdings share a currency; if not, treat as
/// a proxy series. We'll add FX later.)
pub async fn portfolio_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<PortfolioHistoryQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let holdings = state.db.list_holdings().await.map_err(internal)?;
    if holdings.is_empty() {
        return Ok(Json(json!({ "points": [], "cost_basis": 0.0 })));
    }
    let yc = YahooClient::new();
    // Map symbol → total quantity (sum across brokers)
    let mut qty_by_symbol: std::collections::HashMap<String, f64> =
        std::collections::HashMap::new();
    for h in &holdings {
        *qty_by_symbol.entry(h.symbol.clone()).or_insert(0.0) += h.quantity;
    }
    // Total cost basis — flat line for the chart "vs cost" overlay.
    let cost_basis: f64 = holdings
        .iter()
        .map(|h| h.avg_cost_per_unit.unwrap_or(0.0) * h.quantity)
        .sum();

    // If `since_buy`, override the range with the earliest activity timestamp.
    let range = if q.since_buy {
        let earliest: Option<i64> = sqlx::query_scalar(
            "SELECT MIN(timestamp) FROM holding_activities
             WHERE activity_type IN ('BUY','TRANSFER_IN')",
        )
        .fetch_one(&state.db.pool)
        .await
        .ok()
        .flatten();
        match earliest {
            Some(ts) => {
                let days = ((chrono::Utc::now().timestamp() - ts) / 86_400).max(1);
                if days <= 5 {
                    "5d".to_string()
                } else if days <= 30 {
                    "1mo".to_string()
                } else if days <= 90 {
                    "3mo".to_string()
                } else if days <= 365 {
                    "1y".to_string()
                } else if days <= 5 * 365 {
                    "5y".to_string()
                } else {
                    "max".to_string()
                }
            }
            None => q.range.clone(),
        }
    } else {
        q.range.clone()
    };

    // ts → total value
    let mut totals: std::collections::BTreeMap<i64, f64> = std::collections::BTreeMap::new();
    for (symbol, qty) in &qty_by_symbol {
        match yc.history(symbol, &range, &q.interval).await {
            Ok(points) => {
                for p in points {
                    *totals.entry(p.timestamp).or_insert(0.0) += p.close * qty;
                }
            }
            Err(e) => tracing::warn!("history {symbol}: {e:#}"),
        }
    }

    let series: Vec<Value> = totals
        .into_iter()
        .map(|(ts, v)| json!({ "timestamp": ts, "value": v }))
        .collect();
    Ok(Json(json!({ "points": series, "cost_basis": cost_basis })))
}

pub async fn sync_quotes(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let symbols = state.db.distinct_symbols().await.map_err(internal)?;
    if symbols.is_empty() {
        return Ok(Json(json!({ "symbols": 0, "fetched": 0 })));
    }
    let yc = YahooClient::new();
    let refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
    let quotes = yc.quotes(&refs).await.map_err(|e| {
        (StatusCode::BAD_GATEWAY, format!("yahoo: {e:#}"))
    })?;
    let mut count = 0;
    for q in quotes {
        if let Err(e) = state
            .db
            .upsert_latest_quote(
                &q.symbol,
                q.price,
                &q.currency,
                q.previous_close,
                q.day_change_pct,
                q.name.as_deref(),
            )
            .await
        {
            tracing::warn!("upsert quote {}: {e:#}", q.symbol);
        } else {
            count += 1;
        }
    }
    // touch holdings so the UI shows when last we got a price
    for s in &symbols {
        // best-effort
        let _ = sqlx::query(
            "UPDATE holdings SET last_synced_at = strftime('%s','now') WHERE symbol = ?",
        )
        .bind(s)
        .execute(&state.db.pool)
        .await;
    }
    Ok(Json(json!({
        "symbols": symbols.len(),
        "fetched": count,
    })))
}

pub async fn net_worth(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let holdings = state.db.list_holdings().await.map_err(internal)?;
    let quotes = state.db.all_latest_quotes().await.map_err(internal)?;
    let by_symbol: std::collections::HashMap<_, _> =
        quotes.iter().map(|q| (q.symbol.clone(), q)).collect();
    let mut total = 0.0_f64;
    let mut cost = 0.0_f64;
    for h in &holdings {
        if let Some(q) = by_symbol.get(&h.symbol) {
            total += q.price * h.quantity;
        }
        if let Some(c) = h.avg_cost_per_unit {
            cost += c * h.quantity;
        }
    }
    Ok(Json(json!({
        "holdings_market_value": total,
        "holdings_cost_basis": cost,
        "holdings_unrealised_gain": total - cost,
    })))
}
