//! Minimal MCP server over the Streamable-HTTP transport.
//!
//! Exposes a small set of tools so AI clients (Claude / Claude Code / claude.ai custom
//! connector) can query tally. Most tools are read-only; `add_investment_activity` mutates and
//! requires the `write` scope. Single endpoint `POST /mcp` speaking JSON-RPC 2.0; `GET /mcp`
//! returns 405 (no server-initiated SSE stream — not required by the spec). Auth is either an
//! OIDC JWT from the configured IdP (scope-gated) or a tally API token (`Authorization: Bearer
//! <token>`, minted in Settings → full access).

use crate::{oidc, AppState};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;

const PROTOCOL_VERSION: &str = "2024-11-05";

/// GET is not supported (no server-push SSE). Spec allows replying 405.
pub async fn get_handler() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, "MCP: use POST").into_response()
}

pub async fn post_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> Response {
    let auth_ctx = match authed(&state, &headers).await {
        Some(ctx) => ctx,
        None => {
            // 401 + WWW-Authenticate pointing at our protected-resource metadata, so MCP clients
            // can discover the authorization server and start the flow.
            let host = headers
                .get(header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("localhost:3001");
            let scheme = if host.starts_with("localhost") || host.starts_with("127.") {
                "http"
            } else {
                "https"
            };
            let www = format!(
                "Bearer resource_metadata=\"{scheme}://{host}/.well-known/oauth-protected-resource\""
            );
            let mut resp = (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "unauthorized", "error_description": "missing or invalid token"})),
            )
                .into_response();
            if let Ok(v) = axum::http::HeaderValue::from_str(&www) {
                resp.headers_mut().insert(header::WWW_AUTHENTICATE, v);
            }
            return resp;
        }
    };
    let req = match body {
        Some(Json(v)) => v,
        None => return rpc_error_response(Value::Null, -32700, "empty body"),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => rpc_ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "tally", "version": env!("CARGO_PKG_VERSION") }
            }),
        ),
        // Notifications carry no id and expect no JSON-RPC response body.
        m if m.starts_with("notifications/") => StatusCode::ACCEPTED.into_response(),
        "ping" => rpc_ok(id, json!({})),
        "tools/list" => rpc_ok(id, json!({ "tools": tool_defs() })),
        "tools/call" => {
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            if is_write_tool(name) && !auth_ctx.write {
                return rpc_error_response(
                    id,
                    -32000,
                    "this token lacks the 'write' scope required for this tool",
                );
            }
            match call_tool(&state, name, &args).await {
                Ok(v) => rpc_ok(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&v).unwrap_or_default()
                        }]
                    }),
                ),
                Err(e) => rpc_error_response(id, -32000, &e),
            }
        }
        _ => rpc_error_response(id, -32601, "method not found"),
    }
}

/// Authentication outcome for an MCP request. `write` gates mutating tools.
#[derive(Clone, Copy)]
struct AuthCtx {
    write: bool,
}

/// Tools that mutate state and therefore require `write` capability.
fn is_write_tool(name: &str) -> bool {
    matches!(
        name,
        "add_investment_activity" | "add_reminder" | "tick_reminder" | "add_watchlist_item"
    )
}

async fn authed(state: &AppState, headers: &HeaderMap) -> Option<AuthCtx> {
    let h = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok())?;
    let raw = h.strip_prefix("Bearer ")?.trim();
    // OIDC JWT from the external IdP (Authentik/Keycloak) is the ONLY accepted credential on
    // /mcp — no legacy API-token fallback. Reads need any valid token; writes need `write` scope.
    let cfg = state.oidc.as_ref()?;
    let claims = oidc::validate(cfg, raw).await?;
    Some(AuthCtx {
        write: claims.has_scope("write"),
    })
}

fn tool_defs() -> Value {
    json!([
        {
            "name": "list_accounts",
            "description": "List all linked bank and card accounts with current balances (in pence/cents).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_transactions",
            "description": "List recent transactions, newest first. Amounts are in pence/cents; is_credit=1 means money in.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "days": { "type": "integer", "description": "Look back this many days (default 30)." },
                    "limit": { "type": "integer", "description": "Max rows (default 25, max 200)." },
                    "query": { "type": "string", "description": "Case-insensitive substring filter on description." }
                }
            }
        },
        {
            "name": "spending_summary",
            "description": "Total outgoing spend grouped by category id over the last N days.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "days": { "type": "integer", "description": "Window length in days (default 30)." }
                }
            }
        },
        {
            "name": "list_bills",
            "description": "List recurring bills / direct debits, including upcoming due dates and expected amounts.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "within_days": { "type": "integer", "description": "If set, only bills due within this many days." }
                }
            }
        },
        {
            "name": "net_worth",
            "description": "Investment portfolio market value, cost basis and unrealised gain (in major currency units).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_brokers",
            "description": "List investment brokers/accounts (id + name), e.g. to pick one for add_investment_activity.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "add_investment_activity",
            "description": "Record an investment transaction (buy/sell/dividend). Creates the holding on first buy of a symbol. If broker is omitted and only one exists, it is used.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Ticker, e.g. INTC or VWRP.L" },
                    "activity_type": { "type": "string", "description": "BUY | SELL | DIVIDEND | SPLIT | FEE | INTEREST (default BUY)" },
                    "quantity": { "type": "number", "description": "Number of units/shares" },
                    "price_per_unit": { "type": "number", "description": "Price paid per unit (omit for dividend/fee if N/A)" },
                    "date": { "type": "string", "description": "ISO date/datetime, e.g. 2026-05-15 or 2026-05-15T18:24. Defaults to now." },
                    "fee": { "type": "number", "description": "Fee (optional)" },
                    "currency": { "type": "string", "description": "ISO currency, default GBP" },
                    "broker_id": { "type": "integer", "description": "Broker id (optional; see list_brokers)" },
                    "broker_name": { "type": "string", "description": "Broker name to match (optional alternative to broker_id)" },
                    "name": { "type": "string", "description": "Company/asset name (optional)" },
                    "notes": { "type": "string" }
                },
                "required": ["symbol", "quantity"]
            }
        },
        {
            "name": "list_reminders",
            "description": "List active reminders/checklist items (Help to Save, card due dates, etc.) with due dates and whether completed this period.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "add_reminder",
            "description": "Create a recurring reminder/checklist item. freq is hours|days|weeks|months; due_at is the unix timestamp of the first deadline.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "freq": { "type": "string", "description": "hours | days | weeks | months" },
                    "every_n": { "type": "integer", "description": "every N of freq (default 1)" },
                    "anchor_day": { "type": "integer", "description": "day-of-month for monthly, e.g. 28" },
                    "due_at": { "type": "integer", "description": "unix seconds of the first deadline" },
                    "notify_before": { "type": "integer", "description": "seconds before due to notify, e.g. 86400 for a day" },
                    "notes": { "type": "string" }
                },
                "required": ["title", "freq", "due_at"]
            }
        },
        {
            "name": "tick_reminder",
            "description": "Mark a reminder done for the current period.",
            "inputSchema": {
                "type": "object",
                "properties": { "reminder_id": { "type": "integer" } },
                "required": ["reminder_id"]
            }
        },
        {
            "name": "list_watchlist",
            "description": "List deal-watchlist items the user is tracking, with target prices.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "list_deals",
            "description": "List recently found deals/prices across the watchlist, newest first.",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer", "description": "max rows (default 50)" } }
            }
        },
        {
            "name": "add_watchlist_item",
            "description": "Add an item to the deal watchlist with an optional target price (pence) and RSS feed URLs to poll (e.g. a HotUKDeals search.rss URL).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "keywords": { "type": "string" },
                    "target_price_cents": { "type": "integer", "description": "alert when a found price is at/under this (pence)" },
                    "rss_urls": { "type": "array", "items": { "type": "string" }, "description": "RSS feed URLs to poll" }
                },
                "required": ["name"]
            }
        }
    ])
}

/// Parse an ISO date/datetime or unix-seconds string to a unix timestamp; None → now.
fn parse_activity_ts(args: &Value) -> i64 {
    let now = Utc::now().timestamp();
    let Some(s) = args.get("date").and_then(|v| v.as_str()) else {
        return now;
    };
    let s = s.trim();
    if let Ok(n) = s.parse::<i64>() {
        return n;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return dt.timestamp();
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        return dt.and_utc().timestamp();
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
    }
    now
}

async fn call_tool(state: &AppState, name: &str, args: &Value) -> Result<Value, String> {
    let err = |e: anyhow::Error| e.to_string();
    match name {
        "list_accounts" => {
            let accounts = state.db.list_all_enabled_accounts().await.map_err(err)?;
            Ok(json!({ "accounts": accounts }))
        }
        "list_transactions" => {
            let days = args.get("days").and_then(|v| v.as_i64()).unwrap_or(30).clamp(1, 730);
            let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(25).clamp(1, 200);
            let query = args.get("query").and_then(|v| v.as_str());
            let from = Utc::now().timestamp() - days * 86_400;
            let txns = state
                .db
                .list_transactions(None, None, Some(from), None, None, None, None, query, limit, 0)
                .await
                .map_err(err)?;
            Ok(json!({ "transactions": txns }))
        }
        "spending_summary" => {
            let days = args.get("days").and_then(|v| v.as_i64()).unwrap_or(30).clamp(1, 730);
            let now = Utc::now().timestamp();
            let rows = state.db.spending_by_category(now - days * 86_400, now).await.map_err(err)?;
            let summary: Vec<Value> = rows
                .into_iter()
                .map(|(cat, cents)| json!({ "category_id": cat, "total_cents": cents }))
                .collect();
            Ok(json!({ "days": days, "by_category": summary }))
        }
        "list_bills" => {
            let bills = match args.get("within_days").and_then(|v| v.as_i64()) {
                Some(d) => state.db.list_bills_due_within(d.clamp(1, 365)).await.map_err(err)?,
                None => state.db.list_bills().await.map_err(err)?,
            };
            Ok(json!({ "bills": bills }))
        }
        "net_worth" => {
            let holdings = state.db.list_holdings().await.map_err(err)?;
            let quotes = state.db.all_latest_quotes().await.map_err(err)?;
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
            Ok(json!({
                "holdings_market_value": total,
                "holdings_cost_basis": cost,
                "holdings_unrealised_gain": total - cost
            }))
        }
        "list_brokers" => {
            let brokers = state.db.list_brokers().await.map_err(err)?;
            Ok(json!({ "brokers": brokers }))
        }
        "add_investment_activity" => {
            let symbol = args
                .get("symbol")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_uppercase())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "symbol required".to_string())?;
            let quantity = args
                .get("quantity")
                .and_then(|v| v.as_f64())
                .ok_or_else(|| "quantity required".to_string())?;
            let activity_type = args
                .get("activity_type")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_uppercase())
                .unwrap_or_else(|| "BUY".to_string());
            let price = args.get("price_per_unit").and_then(|v| v.as_f64());
            let fee = args.get("fee").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let currency = args
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("GBP")
                .to_uppercase();
            let notes = args.get("notes").and_then(|v| v.as_str());
            let name = args.get("name").and_then(|v| v.as_str());
            let ts = parse_activity_ts(args);

            // Resolve broker: explicit id, else by name, else the sole broker.
            let brokers = state.db.list_brokers().await.map_err(err)?;
            let broker_id = if let Some(id) = args.get("broker_id").and_then(|v| v.as_i64()) {
                id
            } else if let Some(bn) = args.get("broker_name").and_then(|v| v.as_str()) {
                brokers
                    .iter()
                    .find(|b| b.name.eq_ignore_ascii_case(bn.trim()))
                    .map(|b| b.id)
                    .ok_or_else(|| format!("no broker named '{bn}' (use list_brokers)"))?
            } else if brokers.len() == 1 {
                brokers[0].id
            } else {
                return Err(
                    "multiple brokers exist — pass broker_id or broker_name (see list_brokers)".into(),
                );
            };

            let holding_id = state
                .db
                .upsert_holding(broker_id, &symbol, "equity", 0.0, None, &currency, name)
                .await
                .map_err(err)?;
            let activity_id = state
                .db
                .create_activity(holding_id, &activity_type, ts, quantity, price, fee, &currency, notes)
                .await
                .map_err(err)?;
            state
                .db
                .recompute_holding_from_activities(holding_id)
                .await
                .map_err(err)?;
            Ok(json!({
                "ok": true,
                "activity_id": activity_id,
                "holding_id": holding_id,
                "symbol": symbol,
                "activity_type": activity_type,
                "quantity": quantity,
                "price_per_unit": price,
                "broker_id": broker_id,
            }))
        }
        "list_reminders" => {
            let reminders = state.db.list_reminders().await.map_err(err)?;
            Ok(json!({ "reminders": reminders }))
        }
        "add_reminder" => {
            let title = args
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "title required".to_string())?;
            let freq = args.get("freq").and_then(|v| v.as_str()).unwrap_or("days");
            let every_n = args.get("every_n").and_then(|v| v.as_i64()).unwrap_or(1).max(1);
            let anchor_day = args.get("anchor_day").and_then(|v| v.as_i64());
            let due_at = args
                .get("due_at")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "due_at (unix seconds) required".to_string())?;
            let notify_before = args.get("notify_before").and_then(|v| v.as_i64()).unwrap_or(0).max(0);
            let notes = args.get("notes").and_then(|v| v.as_str());
            let id = state
                .db
                .create_reminder(title, notes, freq, every_n, anchor_day, due_at, notify_before, true)
                .await
                .map_err(err)?;
            Ok(json!({ "ok": true, "id": id }))
        }
        "tick_reminder" => {
            let id = args
                .get("reminder_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "reminder_id required".to_string())?;
            state.db.tick_reminder(id).await.map_err(err)?;
            Ok(json!({ "ok": true }))
        }
        "list_watchlist" => {
            let items = state.db.list_watchlist().await.map_err(err)?;
            Ok(json!({ "items": items }))
        }
        "list_deals" => {
            let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
            let deals = state.db.recent_deals(limit).await.map_err(err)?;
            Ok(json!({ "deals": deals }))
        }
        "add_watchlist_item" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "name required".to_string())?;
            let keywords = args.get("keywords").and_then(|v| v.as_str());
            let target = args.get("target_price_cents").and_then(|v| v.as_i64());
            let id = state
                .db
                .create_watchlist_item(name, keywords, target, "GBP")
                .await
                .map_err(err)?;
            let mut added = 0;
            if let Some(urls) = args.get("rss_urls").and_then(|v| v.as_array()) {
                for u in urls.iter().filter_map(|v| v.as_str()) {
                    if state.db.add_watchlist_source(id, "rss", u.trim()).await.is_ok() {
                        added += 1;
                    }
                }
            }
            // Smart default: no feeds given -> auto-watch HotUKDeals for the name.
            if added == 0 {
                let q = match keywords {
                    Some(k) if !k.trim().is_empty() => format!("{name} {}", k.trim()),
                    _ => name.to_string(),
                };
                let feed = crate::routes::api::watchlist::hukd_feed(&q);
                let _ = state.db.add_watchlist_source(id, "rss", &feed).await;
            }
            Ok(json!({ "ok": true, "id": id }))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn rpc_ok(id: Value, result: Value) -> Response {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

fn rpc_error_response(id: Value, code: i64, message: &str) -> Response {
    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    }))
    .into_response()
}
