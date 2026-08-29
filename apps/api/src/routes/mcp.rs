//! Minimal MCP server over the Streamable-HTTP transport.
//!
//! Exposes a small set of tools so AI clients (Claude / Claude Code / claude.ai custom
//! connector) can query tally. Most tools are read-only; mutating tools require the `write`
//! scope. Single endpoint `POST /mcp` speaking JSON-RPC 2.0; `GET /mcp` returns 405 (no
//! server-initiated SSE stream — not required by the spec). The only accepted credential is
//! an OIDC JWT from the configured IdP, taken from `Authorization: Bearer` or, when Cloudflare
//! Access Managed OAuth fronts the endpoint, from `Cf-Access-Jwt-Assertion`.
//!
//! Tool RESULTS are compact markdown, not raw DB JSON — on claude.ai the text content block is
//! the only channel the model reads (server `instructions` are dropped, `structuredContent` is
//! UI-only), and LLMs measurably misread pence integers, unix timestamps and unlabelled signs.
//! All formatting rules live in `mcp_render`.

use super::mcp_render as render;
use crate::models::PlanAccount;
use crate::{oidc, AppState};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Protocol versions we can speak, newest first. We echo the client's requested version when we
/// support it (claude.ai negotiates 2025-06-18; Claude Desktop already sends 2025-11-25 and
/// accepts our newest as a counter-proposal). Never pin old — servers replying 2024-11-05 have
/// been observed to make newer clients list tools but never call them.
const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Injected into the system prompt by clients that honour `initialize.instructions`
/// (Claude Code does; claude.ai currently drops it — the payload conventions repeat there).
const INSTRUCTIONS: &str = "tally is the user's self-hosted personal-finance app (UK, all \
amounts GBP). Start any question about balances, debt, net worth or affordability with \
get_snapshot — it returns every total precomputed. Debts are positive numbers labelled 'owed'. \
Never sum raw rows yourself: quote the precomputed totals, or use the read-only `query` tool \
(SQL over reporting views, money columns in decimal pounds) for custom aggregations. Write \
tools take amounts in pounds, e.g. 12.34.";

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
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let version = if SUPPORTED_VERSIONS.contains(&requested) {
                requested
            } else {
                SUPPORTED_VERSIONS[0]
            };
            rpc_ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "tally",
                        "title": "Tally personal finance",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": INSTRUCTIONS
                }),
            )
        }
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
                Ok(text) => rpc_ok(
                    id,
                    json!({ "content": [{ "type": "text", "text": text }] }),
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
        "add_investment_activity"
            | "upsert_bill"
            | "set_retirement_plan"
            | "categorize_transactions"
            | "add_reminder"
            | "tick_reminder"
            | "upsert_plan_event"
            | "delete_plan_event"
            | "upsert_plan_account"
            | "set_account_balance"
            | "upsert_goal"
            | "set_action_plan"
    )
}

fn pounds_to_cents(v: f64) -> i64 {
    (v * 100.0).round() as i64
}

/// Resolve a plan-account reference (numeric id, numeric string, or name) to its id.
fn match_plan_account(accounts: &[PlanAccount], val: &Value) -> Option<i64> {
    if let Some(id) = val.as_i64() {
        return accounts.iter().find(|a| a.id == id).map(|a| a.id);
    }
    if let Some(s) = val.as_str() {
        let s = s.trim();
        if let Ok(id) = s.parse::<i64>() {
            if let Some(a) = accounts.iter().find(|a| a.id == id) {
                return Some(a.id);
            }
        }
        return accounts.iter().find(|a| a.name.eq_ignore_ascii_case(s)).map(|a| a.id);
    }
    None
}

async fn authed(state: &AppState, headers: &HeaderMap) -> Option<AuthCtx> {
    // Token sources: `Authorization: Bearer` (direct OIDC clients) and
    // `Cf-Access-Jwt-Assertion` (Cloudflare Access Managed OAuth in front of /mcp:
    // Cloudflare runs the OAuth flow for the client and forwards its own JWT, validated
    // here against the team's JWKS like any other issuer). Behind Managed OAuth both
    // headers arrive together — `Authorization` carries the client's Cloudflare-issued
    // access token, which is not the JWT we can verify — so every candidate must be
    // tried, not just the first one present.
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    let cf_assertion = headers
        .get("cf-access-jwt-assertion")
        .and_then(|v| v.to_str().ok());
    // An OIDC JWT from the configured IdP is the ONLY accepted credential on /mcp — no
    // legacy API-token fallback. Reads need any valid token; writes need `write` scope,
    // unless the TALLY_MCP_WRITE self-host escape hatch is set (see oidc::write_allowed_by_env).
    let allow_write_env = oidc::write_allowed_by_env();
    let cfg = state.oidc.as_ref()?;
    for raw in [bearer, cf_assertion].into_iter().flatten() {
        if let Some(claims) = oidc::validate(cfg, raw.trim()).await {
            return Some(AuthCtx {
                write: claims.has_scope("write") || allow_write_env,
            });
        }
    }
    None
}

/// Annotation block for read-only tools (claude.ai groups these as bulk-approvable reads).
fn read_annotations(title: &str) -> Value {
    json!({ "title": title, "readOnlyHint": true, "openWorldHint": false })
}

/// Annotation block for write tools. `destructive` = deletes/replaces data irreversibly.
fn write_annotations(title: &str, destructive: bool) -> Value {
    json!({
        "title": title,
        "readOnlyHint": false,
        "destructiveHint": destructive,
        "openWorldHint": false
    })
}

fn tool_defs() -> Value {
    json!([
        {
            "name": "get_snapshot",
            "title": "Financial snapshot",
            "description": "START HERE for any money question. One call returns the user's whole position with every total precomputed in GBP: TOTAL DEBT (each card/overdraft listed with APR), cash & savings, investments value, net position, safe-to-spend runway, bills due in the next 14 days and goal progress. Answers 'what's my total debt?', 'what's my net worth?', 'how am I doing?' directly — quote the numbers, do not recompute them.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("Financial snapshot")
        },
        {
            "name": "list_accounts",
            "title": "List bank accounts",
            "description": "Live bank & credit-card accounts with formatted GBP balances. Credit cards show what is OWED as a positive amount; overdrawn current accounts say 'overdrawn'. Use for per-account questions; for totals, debt or net worth use get_snapshot.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("List bank accounts")
        },
        {
            "name": "list_transactions",
            "description": "Recent transactions, newest first: ISO dates, in/out direction, formatted GBP amounts, category and account names, with money-in / money-out / net totals for the window precomputed. For custom aggregations (by month, by merchant…) prefer the `query` tool.",
            "title": "List transactions",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "days": { "type": "integer", "description": "Look back this many days (default 30, max 730)." },
                    "limit": { "type": "integer", "description": "Max rows (default 25, max 200)." },
                    "query": { "type": "string", "description": "Case-insensitive substring filter on description." }
                }
            },
            "annotations": read_annotations("List transactions")
        },
        {
            "name": "spending_summary",
            "title": "Spending by category",
            "description": "Outgoing spend per category (names included, sorted biggest first) over the last N days, with the total precomputed in GBP.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "days": { "type": "integer", "description": "Window length in days (default 30)." }
                }
            },
            "annotations": read_annotations("Spending by category")
        },
        {
            "name": "list_bills",
            "title": "List recurring bills",
            "description": "Recurring bills / direct debits with expected GBP amounts, frequency and next due dates, plus the expected total precomputed. Set within_days to only see bills due soon.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "within_days": { "type": "integer", "description": "If set, only bills due within this many days." }
                }
            },
            "annotations": read_annotations("List recurring bills")
        },
        {
            "name": "upsert_bill",
            "title": "Create or update a bill",
            "description": "Create or update a recurring bill/direct debit. Matches an existing bill by bill_id or exact name (case-insensitive), otherwise creates one. Amounts in pounds (GBP); pass expected for a fixed amount or expected_min/expected_max for a range. Set enabled=false to hide a bill (e.g. a cancelled or wrongly detected mandate).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Bill name, e.g. 'Virgin Media'. Used to match an existing bill if bill_id is not given." },
                    "bill_id": { "type": "integer", "description": "Existing bill id to update (see list_bills / v_bills)." },
                    "expected": { "type": "number", "description": "Expected amount in pounds (sets min = max)." },
                    "expected_min": { "type": "number", "description": "Lower bound in pounds (alternative to expected)." },
                    "expected_max": { "type": "number", "description": "Upper bound in pounds (alternative to expected)." },
                    "repeat_freq": { "type": "string", "description": "weekly | fortnightly | monthly | yearly (default monthly; only used when creating)" },
                    "next_due": { "type": "string", "description": "ISO date of the next expected payment, e.g. 2026-08-01." },
                    "match_regex": { "type": "string", "description": "Regex matched against transaction descriptions to auto-link payments (optional)." },
                    "enabled": { "type": "boolean", "description": "false hides the bill from lists and totals." }
                },
                "required": ["name"]
            },
            "annotations": write_annotations("Create or update a bill", false)
        },
        {
            "name": "investments_summary",
            "title": "Investments summary",
            "description": "Investment portfolio ONLY (this is NOT overall net worth — use get_snapshot for that): each holding's value and gain, plus total market value, cost basis and unrealised gain in GBP.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("Investments summary")
        },
        {
            "name": "retirement_forecast",
            "title": "Retirement forecast",
            "description": "Am-I-on-track retirement maths: target retirement age vs the invested pension pot (SIPP/SCAVC holdings). Shows the pot required to bridge until the LGPS defined benefit (~67) and state pension start, the projected pot at target age, and the monthly saving needed. All GBP, today's money.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("Retirement forecast")
        },
        {
            "name": "set_retirement_plan",
            "title": "Set retirement plan",
            "description": "Update retirement-forecast assumptions (any subset). Fields: birth_date (ISO), target_age, target_income_annual (£/yr today's money), growth_pct (REAL growth after inflation), salary_annual (£), lgps_start (ISO date LGPS membership began), lgps_age, state_pension_annual, state_pension_age, monthly_contribution (£/month into SIPP/SCAVCs), include_general_investments (bool). Returns the recomputed forecast.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "birth_date": { "type": "string", "description": "ISO date of birth, e.g. 2000-03-14" },
                    "target_age": { "type": "number", "description": "Age you want to retire, e.g. 57" },
                    "target_income_annual": { "type": "number", "description": "Desired retirement income, £/year in today's money" },
                    "growth_pct": { "type": "number", "description": "Real (after-inflation) annual growth %, default 5" },
                    "salary_annual": { "type": "number", "description": "Current gross salary £ (drives LGPS accrual)" },
                    "lgps_start": { "type": "string", "description": "ISO date LGPS membership started" },
                    "lgps_age": { "type": "number", "description": "Age LGPS starts paying (default 67)" },
                    "state_pension_annual": { "type": "number", "description": "£/year, default 12000" },
                    "state_pension_age": { "type": "number", "description": "default 68" },
                    "monthly_contribution": { "type": "number", "description": "Current £/month going into SIPP/SCAVCs" },
                    "include_general_investments": { "type": "boolean", "description": "Count non-pension holdings in the pot too" },
                    "notes": { "type": "string", "description": "Replace the free-form pension notes markdown shown on the Retirement page (read it via retirement_forecast first; this REPLACES the whole text)" }
                }
            },
            "annotations": write_annotations("Set retirement plan", false)
        },
        {
            "name": "categorize_transactions",
            "title": "Categorise transactions",
            "description": "Tag one or more transactions with a category (find ids + current categories via list_transactions or the query tool over v_transactions; category names via v_categories). Pass category 'none' to clear. Use this to quickly tidy uncategorised spending from chat.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "transaction_ids": { "type": "array", "items": { "type": "integer" }, "description": "Transaction ids to tag" },
                    "category": { "type": "string", "description": "Existing category name (case-insensitive), or 'none' to clear" }
                },
                "required": ["transaction_ids", "category"]
            },
            "annotations": write_annotations("Categorise transactions", false)
        },
        {
            "name": "get_safe_to_spend",
            "title": "Safe to spend",
            "description": "Forward-looking 'safe to spend today' runway to the next payday, all in GBP: safe amount today, per-day rate, spendable above each account's floor, committed bills before payday, days left and the payday date. Says so if not configured yet.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("Safe to spend")
        },
        {
            "name": "query",
            "title": "Read-only SQL query",
            "description": "Run ONE read-only SQL SELECT (SQLite dialect) over tally's reporting views — for questions no other tool answers directly (month-by-month trends, merchant totals, custom filters). All money columns are decimal POUNDS (named *_pounds), all dates ISO strings, results capped at 200 rows. Views: v_transactions(id, date, description, merchant_name, direction 'in'|'out', amount_pounds, signed_pounds [in=+, out=-], is_pending, category, account, currency) · v_accounts(id, name, type 'current_account'|'credit_card', balance_pounds [cards: positive = owed], available_pounds, overdraft_limit_pounds, credit_limit_pounds, payment_due_pounds, payment_due_date, balance_updated) · v_plan_accounts(id, name, kind, source, balance_pounds [negative = owed], floor_pounds, credit_limit_pounds, apr_percent, statement_day, cliff_date, cliff_new_floor_pounds) · v_plan_events(id, date, label, source, signed_pounds, amount_pounds, direction, account, to_account, recurrence, recur_until, note) · v_bills(id, name, expected_min_pounds, expected_max_pounds, repeat_freq, next_due, last_paid) · v_goals(id, name, target_pounds, saved_pounds, monthly_pounds, target_date) · v_holdings(id, broker, symbol, name, quantity, avg_cost_per_unit, last_price, market_value_pounds, cost_basis_pounds) · v_categories(id, name, parent_id) · v_net_worth_history(day, cash_pounds, debt_pounds, investments_pounds, pension_pounds, net_pounds — one row per day, recorded daily from 2026-07-18). Example: SELECT substr(date,1,7) AS month, ROUND(SUM(amount_pounds),2) AS spent FROM v_transactions WHERE direction='out' GROUP BY month ORDER BY month DESC LIMIT 6",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "A single SELECT (or WITH … SELECT) statement." }
                },
                "required": ["sql"]
            },
            "annotations": read_annotations("Read-only SQL query")
        },
        {
            "name": "list_brokers",
            "title": "List brokers",
            "description": "Investment brokers/accounts (id + name), e.g. to pick one for add_investment_activity.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("List brokers")
        },
        {
            "name": "add_investment_activity",
            "title": "Record investment activity",
            "description": "Record an investment transaction (buy/sell/dividend). Creates the holding on first buy of a symbol. Amounts in pounds (GBP). If broker is omitted and only one exists, it is used.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Ticker, e.g. INTC or VWRP.L" },
                    "activity_type": { "type": "string", "description": "BUY | SELL | DIVIDEND | SPLIT | FEE | INTEREST (default BUY)" },
                    "quantity": { "type": "number", "description": "Number of units/shares" },
                    "price_per_unit": { "type": "number", "description": "Price paid per unit in pounds (omit for dividend/fee if N/A)" },
                    "date": { "type": "string", "description": "ISO date/datetime, e.g. 2026-05-15 or 2026-05-15T18:24. Defaults to now." },
                    "fee": { "type": "number", "description": "Fee in pounds (optional)" },
                    "currency": { "type": "string", "description": "ISO currency, default GBP" },
                    "broker_id": { "type": "integer", "description": "Broker id (optional; see list_brokers)" },
                    "broker_name": { "type": "string", "description": "Broker name to match (optional alternative to broker_id)" },
                    "name": { "type": "string", "description": "Company/asset name (optional)" },
                    "notes": { "type": "string" }
                },
                "required": ["symbol", "quantity"]
            },
            "annotations": write_annotations("Record investment activity", false)
        },
        {
            "name": "list_reminders",
            "title": "List reminders",
            "description": "Active reminders/checklist items (Help to Save, card due dates, etc.) with ISO due dates, repeat cadence and when each was last done.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("List reminders")
        },
        {
            "name": "add_reminder",
            "title": "Add reminder",
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
            },
            "annotations": write_annotations("Add reminder", false)
        },
        {
            "name": "tick_reminder",
            "title": "Tick reminder",
            "description": "Mark a reminder done for the current period.",
            "inputSchema": {
                "type": "object",
                "properties": { "reminder_id": { "type": "integer" } },
                "required": ["reminder_id"]
            },
            "annotations": write_annotations("Tick reminder", false)
        },
        {
            "name": "list_ahead",
            "title": "Cashflow forecast (Ahead)",
            "description": "The 'Ahead' cashflow forecast: planning accounts with current balance, floor and projected low point; dated upcoming events (bills, salary, transfers) over the next N days; 0%-buffer cliff warnings; goals; and the current 'Moves to make' action plan. Use this to see the runway before adding or moving money. All amounts GBP.",
            "inputSchema": {
                "type": "object",
                "properties": { "days": { "type": "integer", "description": "Days to project forward (default 60)." } }
            },
            "annotations": read_annotations("Cashflow forecast (Ahead)")
        },
        {
            "name": "list_plan_accounts",
            "title": "List planning accounts",
            "description": "Planning accounts (id, name, kind, balance, floor, credit limit, APR) — includes manual accounts Open Banking can't reach. Use to pick the `account` for upsert_plan_event / set_account_balance. Amounts owed shown as positive 'owed' figures.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("List planning accounts")
        },
        {
            "name": "upsert_plan_event",
            "title": "Add/edit plan event",
            "description": "Add or edit a dated money event on the Ahead forecast. direction 'out'/'in' is a one-account cashflow; 'transfer' moves money from `account` into `to_account`. Amounts are in POUNDS (GBP), e.g. 950 or 12.34. Pass `id` to edit an existing event.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Existing event id to edit (omit to create)." },
                    "date": { "type": "string", "description": "ISO date YYYY-MM-DD of the (first) occurrence." },
                    "label": { "type": "string", "description": "e.g. 'Rent', 'Car repair', 'Refill 0% overdraft'." },
                    "amount": { "type": "number", "description": "Amount in pounds (magnitude)." },
                    "direction": { "type": "string", "description": "out | in | transfer (default out)." },
                    "account": { "type": "string", "description": "Account name or id (the source for a transfer)." },
                    "to_account": { "type": "string", "description": "Destination account name or id (required for transfer)." },
                    "recurrence": { "type": "string", "description": "none | weekly | fortnightly | monthly | yearly (default none)." },
                    "note": { "type": "string" }
                },
                "required": ["date", "label", "amount", "account"]
            },
            "annotations": write_annotations("Add/edit plan event", false)
        },
        {
            "name": "delete_plan_event",
            "title": "Delete plan event",
            "description": "Delete a plan event from the Ahead forecast by id.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "integer" } },
                "required": ["id"]
            },
            "annotations": write_annotations("Delete plan event", true)
        },
        {
            "name": "upsert_plan_account",
            "title": "Add/edit planning account",
            "description": "Add a MANUAL planning account (e.g. a credit card Open Banking can't sync) or edit any planning account's metadata. Amounts in POUNDS (GBP); apr is a percent (e.g. 34.9). Pass `id` to edit. For synced accounts only the planning metadata (floor/apr/limit/cliff) is changed, not the live balance.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Existing plan-account id to edit (omit to create a manual one)." },
                    "name": { "type": "string" },
                    "kind": { "type": "string", "description": "current | savings | credit | cash" },
                    "balance": { "type": "number", "description": "Balance in pounds (negative = owed/overdrawn). Manual accounts only." },
                    "floor": { "type": "number", "description": "Lowest allowed balance in pounds (negative for an overdraft buffer)." },
                    "credit_limit": { "type": "number", "description": "Credit limit in pounds (cards)." },
                    "apr": { "type": "number", "description": "APR percent, e.g. 34.9." },
                    "statement_day": { "type": "integer", "description": "Day of month the statement issues." },
                    "cliff_date": { "type": "string", "description": "ISO date a 0% buffer ends (floor flips then)." },
                    "cliff_new_floor": { "type": "number", "description": "Floor in pounds from the cliff date onward." },
                    "overflow_account": { "type": "string", "description": "Account name or id that absorbs this account's below-floor shortfall (e.g. point a no-overdraft current account at an overdraft account so it floors at £0 and the deficit lands there). Pass 'none' to clear." }
                }
            },
            "annotations": write_annotations("Add/edit planning account", false)
        },
        {
            "name": "set_account_balance",
            "title": "Set manual account balance",
            "description": "Set the current balance (in POUNDS, GBP) of a MANUAL planning account, e.g. after paying a credit card. Negative = owed/overdrawn.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "account": { "type": "string", "description": "Account name or id." },
                    "balance": { "type": "number", "description": "New balance in pounds." }
                },
                "required": ["account", "balance"]
            },
            "annotations": write_annotations("Set manual account balance", false)
        },
        {
            "name": "list_goals",
            "title": "List savings goals",
            "description": "Savings goals with target, saved-so-far, % progress and monthly contribution — totals precomputed, in GBP.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_annotations("List savings goals")
        },
        {
            "name": "upsert_goal",
            "title": "Add/edit savings goal",
            "description": "Add or edit a savings goal. Amounts in POUNDS (GBP). Pass `id` to edit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                    "name": { "type": "string" },
                    "target": { "type": "number", "description": "Target amount in pounds." },
                    "saved": { "type": "number", "description": "Saved so far, in pounds." },
                    "target_date": { "type": "string", "description": "ISO date." },
                    "monthly": { "type": "number", "description": "Planned monthly contribution in pounds." }
                },
                "required": ["name", "target"]
            },
            "annotations": write_annotations("Add/edit savings goal", false)
        },
        {
            "name": "set_action_plan",
            "title": "Set action plan",
            "description": "Set the plain-English 'Moves to make' checklist shown at the top of the Ahead page — the manual transfers and to-dos the user needs to action (e.g. account-to-account moves Tally can't do automatically). REPLACES the whole list. Use markdown: lines starting with '- ' become tickable checklist items; other lines are headings/notes. Keep it short, dated, and concrete. Read the current one via list_ahead first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The full checklist in markdown. Bullet lines ('- ...') become tick items." }
                },
                "required": ["text"]
            },
            "annotations": write_annotations("Set action plan", true)
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

/// Execute a validated read-only SELECT and render the result as a markdown table.
async fn run_query(state: &AppState, sql: &str) -> Result<String, String> {
    let sql = render::validate_query(sql)?;
    // Wrap so a row cap applies no matter what LIMIT the inner query carries.
    let wrapped = format!("SELECT * FROM ({sql}) LIMIT 201");
    let fut = state.db.conn.query(&wrapped, ());
    let mut rows = tokio::time::timeout(std::time::Duration::from_secs(10), fut)
        .await
        .map_err(|_| "query timed out (10s)".to_string())?
        .map_err(|e| format!("SQL error: {e}"))?;

    let ncols = rows.column_count();
    let headers: Vec<String> = (0..ncols)
        .map(|i| rows.column_name(i).unwrap_or("?").to_string())
        .collect();
    let mut data: Vec<Vec<String>> = Vec::new();
    let mut truncated = false;
    while let Some(row) = rows.next().await.map_err(|e| format!("SQL error: {e}"))? {
        if data.len() >= 200 {
            truncated = true;
            break;
        }
        let mut r = Vec::with_capacity(ncols as usize);
        for i in 0..ncols {
            let v = row.get_value(i).map_err(|e| e.to_string())?;
            r.push(match v {
                libsql::Value::Null => String::new(),
                libsql::Value::Integer(n) => n.to_string(),
                libsql::Value::Real(f) => format!("{f}"),
                libsql::Value::Text(t) => t,
                libsql::Value::Blob(_) => "<blob>".into(),
            });
        }
        data.push(r);
    }
    if data.is_empty() {
        return Ok("0 rows.".into());
    }
    let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
    Ok(format!(
        "{} rows{}.\n\n{}\n_Money columns are decimal pounds (GBP)._",
        data.len(),
        if truncated { " (capped at 200 — the full result is bigger; aggregate or filter)" } else { "" },
        render::md_table(&header_refs, &data),
    ))
}

async fn call_tool(state: &AppState, name: &str, args: &Value) -> Result<String, String> {
    let err = |e: anyhow::Error| e.to_string();
    match name {
        "get_snapshot" => {
            let accounts = state.db.list_all_enabled_accounts().await.map_err(err)?;
            let plans = state.db.list_plan_accounts().await.map_err(err)?;
            let holdings = state.db.list_holdings().await.map_err(err)?;
            let quotes = state.db.all_latest_quotes().await.map_err(err)?;
            // Cheap, TTL-gated: make sure GBP FX rates exist for the currencies in play
            // so the investments line is converted, then re-read the cache.
            let ccys = holdings
                .iter()
                .map(|h| h.currency.clone())
                .chain(quotes.iter().map(|q| q.currency.clone()));
            crate::fx::ensure_fx_quotes(state, ccys).await;
            let quotes = state.db.all_latest_quotes().await.map_err(err)?;
            let brokers = state.db.list_brokers().await.map_err(err)?;
            let sts = crate::routes::api::safe_to_spend::compute(state).await.map_err(err)?;
            let bills = state.db.list_bills_due_within(14).await.map_err(err)?;
            let goals = state.db.list_goals().await.map_err(err)?;
            let today = Utc::now().date_naive().to_string();
            let mut text = render::render_snapshot(
                &today, &accounts, &plans, &holdings, &quotes, &brokers, &sts, &bills, &goals,
            );
            // Trend line from the recorded daily history (started 2026-07-18) — how the
            // net position moved over the window we have, up to ~30 days.
            if let Ok(hist) = state.db.list_net_worth_history(31).await {
                if let (Some(first), Some(last)) = (hist.first(), hist.last()) {
                    if first.0 != last.0 {
                        let delta = last.5 - first.5;
                        let word = if delta >= 0 { "improved" } else { "worsened" };
                        text.push_str(&format!(
                            "\n_Net position has {word} by {} since {} (see v_net_worth_history for the daily series)._\n",
                            render::gbp(delta.abs()),
                            first.0,
                        ));
                    }
                }
            }
            Ok(text)
        }
        "list_accounts" => {
            let accounts = state.db.list_all_enabled_accounts().await.map_err(err)?;
            Ok(render::render_accounts(&accounts))
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
            let accounts = state.db.list_all_enabled_accounts().await.map_err(err)?;
            let names: HashMap<i64, String> =
                accounts.iter().map(|a| (a.id, render::account_name(a))).collect();
            let cats: HashMap<i64, String> = state
                .db
                .list_categories()
                .await
                .map_err(err)?
                .into_iter()
                .map(|c| (c.id, c.name))
                .collect();
            Ok(render::render_transactions(&txns, &names, &cats, days, limit))
        }
        "spending_summary" => {
            let days = args.get("days").and_then(|v| v.as_i64()).unwrap_or(30).clamp(1, 730);
            let now = Utc::now().timestamp();
            let rows = state.db.spending_by_category(now - days * 86_400, now).await.map_err(err)?;
            let cats: HashMap<i64, String> = state
                .db
                .list_categories()
                .await
                .map_err(err)?
                .into_iter()
                .map(|c| (c.id, c.name))
                .collect();
            Ok(render::render_spending(&rows, &cats, days))
        }
        "list_bills" => {
            let within = args.get("within_days").and_then(|v| v.as_i64()).map(|d| d.clamp(1, 365));
            let bills = match within {
                Some(d) => state.db.list_bills_due_within(d).await.map_err(err)?,
                None => state.db.list_bills().await.map_err(err)?,
            };
            Ok(render::render_bills(&bills, within))
        }
        "upsert_bill" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "name required".to_string())?;
            let bills = state.db.list_bills().await.map_err(err)?;
            let existing = if let Some(id) = args.get("bill_id").and_then(|v| v.as_i64()) {
                Some(bills.iter().find(|b| b.id == id).ok_or_else(|| {
                    format!("no bill with id {id} (use list_bills)")
                })?)
            } else {
                bills.iter().find(|b| b.name.trim().eq_ignore_ascii_case(name))
            };
            let expected = args.get("expected").and_then(|v| v.as_f64());
            let min = args
                .get("expected_min")
                .and_then(|v| v.as_f64())
                .or(expected)
                .map(pounds_to_cents);
            let max = args
                .get("expected_max")
                .and_then(|v| v.as_f64())
                .or(expected)
                .map(pounds_to_cents);
            let next_due = args.get("next_due").and_then(|v| v.as_str()).map(|s| {
                let owned = json!({ "date": s });
                parse_activity_ts(&owned)
            });
            let regex = args.get("match_regex").and_then(|v| v.as_str());
            let enabled = args.get("enabled").and_then(|v| v.as_bool());
            match existing {
                Some(b) => {
                    state
                        .db
                        .update_bill(b.id, Some(name), min, max, next_due, regex, enabled)
                        .await
                        .map_err(err)?;
                    Ok(format!("Updated bill #{} ({name}).", b.id))
                }
                None => {
                    let freq = args
                        .get("repeat_freq")
                        .and_then(|v| v.as_str())
                        .unwrap_or("monthly");
                    let id = state
                        .db
                        .create_bill(
                            name,
                            min.unwrap_or(0),
                            max.or(min).unwrap_or(0),
                            "GBP",
                            freq,
                            next_due,
                            regex,
                            None,
                        )
                        .await
                        .map_err(err)?;
                    if enabled == Some(false) {
                        state
                            .db
                            .update_bill(id, None, None, None, None, None, Some(false))
                            .await
                            .map_err(err)?;
                    }
                    Ok(format!("Created {freq} bill #{id} ({name})."))
                }
            }
        }
        "investments_summary" => {
            let holdings = state.db.list_holdings().await.map_err(err)?;
            // Refresh stale prices + FX so chat sees live GBP values, same as the UI.
            crate::routes::api::holdings::refresh_stale_quotes(state, &holdings).await;
            let quotes = state.db.all_latest_quotes().await.map_err(err)?;
            let brokers = state.db.list_brokers().await.map_err(err)?;
            Ok(render::render_investments(&holdings, &quotes, &brokers))
        }
        "retirement_forecast" => {
            let (plan, pot) = crate::routes::api::retirement::load_plan_and_pot(state)
                .await
                .map_err(err)?;
            let f = crate::routes::api::retirement::compute_forecast(&plan, pot);
            let mut out = render::render_retirement(&plan, &f);
            let notes = state
                .db
                .get_setting(crate::routes::api::retirement::NOTES_KEY)
                .await
                .map_err(err)?
                .unwrap_or_default();
            if !notes.trim().is_empty() {
                out.push_str(&format!(
                    "\n\n## Pension notes (user-editable, update with set_retirement_plan `notes`)\n\n{notes}"
                ));
            }
            Ok(out)
        }
        "set_retirement_plan" => {
            // Free-form notes live in settings, not the plan JSON — handle first and
            // strip so the merge below only sees plan fields.
            let mut args = args.clone();
            if let Some(notes) = args
                .as_object_mut()
                .and_then(|o| o.remove("notes"))
                .and_then(|v| v.as_str().map(|s| s.to_string()))
            {
                state
                    .db
                    .set_setting(crate::routes::api::retirement::NOTES_KEY, &notes)
                    .await
                    .map_err(err)?;
                if args.as_object().map(|o| o.is_empty()).unwrap_or(true) {
                    return Ok("Pension notes updated.".to_string());
                }
            }
            let args = &args;
            let current = state.db.get_retirement_plan_json().await.map_err(err)?;
            let mut merged: Value = current
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_else(|| {
                    serde_json::to_value(crate::routes::api::retirement::RetirementPlan::default())
                        .unwrap_or_default()
                });
            if let (Some(obj), Some(patch)) = (merged.as_object_mut(), args.as_object()) {
                for (k, v) in patch {
                    obj.insert(k.clone(), v.clone());
                }
            }
            let plan: crate::routes::api::retirement::RetirementPlan =
                serde_json::from_value(merged).map_err(|e| format!("invalid plan: {e}"))?;
            let json = serde_json::to_string(&plan).map_err(|e| e.to_string())?;
            state.db.set_retirement_plan_json(&json).await.map_err(err)?;
            let (plan, pot) = crate::routes::api::retirement::load_plan_and_pot(state)
                .await
                .map_err(err)?;
            let f = crate::routes::api::retirement::compute_forecast(&plan, pot);
            Ok(format!(
                "Plan saved.\n\n{}",
                render::render_retirement(&plan, &f)
            ))
        }
        "categorize_transactions" => {
            let ids: Vec<i64> = args
                .get("transaction_ids")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
                .unwrap_or_default();
            if ids.is_empty() {
                return Err("transaction_ids (array of ids from list_transactions / v_transactions) required".into());
            }
            let cat_arg = args
                .get("category")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .ok_or_else(|| "category required (a category name, or 'none' to clear)".to_string())?;
            let category_id: Option<i64> = if cat_arg.eq_ignore_ascii_case("none")
                || cat_arg.eq_ignore_ascii_case("uncategorised")
                || cat_arg.eq_ignore_ascii_case("uncategorized")
            {
                None
            } else {
                let cats = state.db.list_categories().await.map_err(err)?;
                Some(
                    cats.iter()
                        .find(|c| c.name.eq_ignore_ascii_case(cat_arg))
                        .map(|c| c.id)
                        .ok_or_else(|| {
                            format!(
                                "no category named '{cat_arg}'. Existing: {}",
                                cats.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
                            )
                        })?,
                )
            };
            let mut updated = 0;
            for id in &ids {
                state
                    .db
                    .update_transaction_category(*id, category_id)
                    .await
                    .map_err(err)?;
                updated += 1;
            }
            Ok(match category_id {
                Some(_) => format!("Tagged {updated} transaction(s) as '{cat_arg}'."),
                None => format!("Cleared the category on {updated} transaction(s)."),
            })
        }
        "get_safe_to_spend" => {
            let v = crate::routes::api::safe_to_spend::compute(state).await.map_err(err)?;
            Ok(render::render_safe_to_spend(&v))
        }
        "query" => {
            let sql = args
                .get("sql")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "sql required".to_string())?;
            run_query(state, sql).await
        }
        "list_brokers" => {
            let brokers = state.db.list_brokers().await.map_err(err)?;
            Ok(render::render_brokers(&brokers))
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
            Ok(format!(
                "Recorded {activity_type} of {quantity} × {symbol}{} (activity #{activity_id}, holding #{holding_id}).",
                price.map(|p| format!(" @ {}", render::gbp_f(p))).unwrap_or_default(),
            ))
        }
        "list_reminders" => {
            let reminders = state.db.list_reminders().await.map_err(err)?;
            Ok(render::render_reminders(&reminders))
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
            Ok(format!(
                "Reminder #{id} '{title}' created, first due {}.",
                render::iso_date(due_at)
            ))
        }
        "tick_reminder" => {
            let id = args
                .get("reminder_id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| "reminder_id required".to_string())?;
            state.db.tick_reminder(id).await.map_err(err)?;
            Ok(format!("Reminder #{id} marked done for this period."))
        }
        "list_ahead" => {
            let days = args.get("days").and_then(|v| v.as_i64()).unwrap_or(60).clamp(7, 730);
            let v = crate::routes::api::ahead::build_forecast(state, days).await.map_err(err)?;
            Ok(render::render_ahead(&v))
        }
        "list_plan_accounts" => {
            let accounts = state.db.list_plan_accounts().await.map_err(err)?;
            Ok(render::render_plan_accounts(&accounts))
        }
        "upsert_plan_event" => {
            let date = args
                .get("date")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "date (YYYY-MM-DD) required".to_string())?;
            let label = args
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "label required".to_string())?;
            let amount = args.get("amount").and_then(|v| v.as_f64()).ok_or_else(|| "amount required".to_string())?;
            let cents = pounds_to_cents(amount.abs());
            let direction = args.get("direction").and_then(|v| v.as_str()).unwrap_or("out").to_lowercase();
            let recurrence = args.get("recurrence").and_then(|v| v.as_str()).unwrap_or("none").to_string();
            let note = args.get("note").and_then(|v| v.as_str());

            let accounts = state.db.list_plan_accounts().await.map_err(err)?;
            let account_id = args
                .get("account")
                .and_then(|v| match_plan_account(&accounts, v))
                .ok_or_else(|| "unknown 'account' (use list_plan_accounts)".to_string())?;
            let (acc, to_acc, amount_cents) = match direction.as_str() {
                "in" => (Some(account_id), None, cents),
                "transfer" => {
                    let to = args
                        .get("to_account")
                        .and_then(|v| match_plan_account(&accounts, v))
                        .ok_or_else(|| "transfer needs a valid 'to_account'".to_string())?;
                    (Some(account_id), Some(to), cents)
                }
                _ => (Some(account_id), None, -cents), // "out"
            };

            if let Some(id) = args.get("id").and_then(|v| v.as_i64()) {
                let cur = state.db.get_plan_event(id).await.map_err(err)?.ok_or_else(|| "event not found".to_string())?;
                state
                    .db
                    .update_plan_event(
                        id, &date, &label, "llm", acc, to_acc, amount_cents, &recurrence,
                        cur.recur_until.as_deref(), cur.category_id, cur.match_regex.as_deref(), note, 1,
                    )
                    .await
                    .map_err(err)?;
                Ok(format!("Plan event #{id} updated: '{label}' on {date}."))
            } else {
                let id = state
                    .db
                    .create_plan_event(&date, &label, "llm", acc, to_acc, amount_cents, &recurrence, None, None, None, note)
                    .await
                    .map_err(err)?;
                Ok(format!("Plan event #{id} created: '{label}' on {date}."))
            }
        }
        "delete_plan_event" => {
            let id = args.get("id").and_then(|v| v.as_i64()).ok_or_else(|| "id required".to_string())?;
            state.db.delete_plan_event(id).await.map_err(err)?;
            Ok(format!("Plan event #{id} deleted."))
        }
        "upsert_plan_account" => {
            let balance = args.get("balance").and_then(|v| v.as_f64()).map(pounds_to_cents);
            let floor = args.get("floor").and_then(|v| v.as_f64()).map(pounds_to_cents);
            let credit_limit = args.get("credit_limit").and_then(|v| v.as_f64()).map(pounds_to_cents);
            let apr_bps = args.get("apr").and_then(|v| v.as_f64()).map(|p| (p * 100.0).round() as i64);
            let statement_day = args.get("statement_day").and_then(|v| v.as_i64());
            let cliff_date = args.get("cliff_date").and_then(|v| v.as_str());
            let cliff_new_floor = args.get("cliff_new_floor").and_then(|v| v.as_f64()).map(pounds_to_cents);
            let kind = args.get("kind").and_then(|v| v.as_str()).unwrap_or("current");

            if let Some(id) = args.get("id").and_then(|v| v.as_i64()) {
                let cur = state.db.get_plan_account(id).await.map_err(err)?.ok_or_else(|| "account not found".to_string())?;
                let name = args.get("name").and_then(|v| v.as_str()).unwrap_or(&cur.name);
                // synced accounts keep their live balance
                let bal = if cur.source == "manual" { balance.unwrap_or(cur.balance_cents) } else { cur.balance_cents };
                state
                    .db
                    .update_plan_account(
                        id,
                        name,
                        args.get("kind").and_then(|v| v.as_str()).unwrap_or(&cur.kind),
                        bal,
                        &cur.currency,
                        floor.unwrap_or(cur.floor_cents),
                        cliff_date.or(cur.cliff_date.as_deref()),
                        cliff_new_floor.or(cur.cliff_new_floor_cents),
                        credit_limit.or(cur.credit_limit_cents),
                        apr_bps.or(cur.apr_bps),
                        statement_day.or(cur.statement_day),
                        cur.payment_intent.as_deref(),
                        cur.sort_order,
                        cur.enabled,
                    )
                    .await
                    .map_err(err)?;
                // Optional: set or clear the floor-overflow link ('none' clears).
                if let Some(ov) = args.get("overflow_account") {
                    let is_clear = matches!(ov, Value::String(s)
                        if s.trim().eq_ignore_ascii_case("none") || s.trim().is_empty());
                    let target = if is_clear {
                        None
                    } else {
                        let accts = state.db.list_plan_accounts().await.map_err(err)?;
                        Some(match_plan_account(&accts, ov).ok_or_else(|| format!("overflow_account not found: {ov}"))?)
                    };
                    state.db.set_plan_account_overflow(id, target).await.map_err(err)?;
                }
                Ok(format!("Planning account #{id} updated."))
            } else {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "name required to create an account".to_string())?;
                let id = state
                    .db
                    .create_plan_account(
                        name, kind, "manual", None, balance.unwrap_or(0), "GBP",
                        floor.unwrap_or(0), credit_limit, apr_bps, statement_day,
                    )
                    .await
                    .map_err(err)?;
                Ok(format!("Manual planning account #{id} '{name}' created."))
            }
        }
        "set_account_balance" => {
            let balance = args.get("balance").and_then(|v| v.as_f64()).ok_or_else(|| "balance required".to_string())?;
            let accounts = state.db.list_plan_accounts().await.map_err(err)?;
            let id = args
                .get("account")
                .and_then(|v| match_plan_account(&accounts, v))
                .ok_or_else(|| "unknown 'account' (use list_plan_accounts)".to_string())?;
            state.db.set_plan_account_balance(id, pounds_to_cents(balance)).await.map_err(err)?;
            Ok(format!(
                "Planning account #{id} balance set to {}.",
                render::gbp(pounds_to_cents(balance))
            ))
        }
        "list_goals" => {
            let goals = state.db.list_goals().await.map_err(err)?;
            Ok(render::render_goals(&goals))
        }
        "set_action_plan" => {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("");
            state.db.set_setting("ahead_action_plan", text).await.map_err(err)?;
            Ok("Action plan replaced.".into())
        }
        "upsert_goal" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "name required".to_string())?;
            let target = args.get("target").and_then(|v| v.as_f64()).ok_or_else(|| "target required".to_string())?;
            let target_cents = pounds_to_cents(target);
            let saved = args.get("saved").and_then(|v| v.as_f64()).map(pounds_to_cents);
            let target_date = args.get("target_date").and_then(|v| v.as_str());
            let monthly = args.get("monthly").and_then(|v| v.as_f64()).map(pounds_to_cents);

            if let Some(id) = args.get("id").and_then(|v| v.as_i64()) {
                let cur = state.db.get_goal(id).await.map_err(err)?.ok_or_else(|| "goal not found".to_string())?;
                state
                    .db
                    .update_goal(
                        id, &name, target_cents, saved.unwrap_or(cur.saved_cents),
                        cur.source_account_id, target_date.or(cur.target_date.as_deref()),
                        monthly.unwrap_or(cur.monthly_cents), 1,
                    )
                    .await
                    .map_err(err)?;
                Ok(format!("Goal #{id} '{name}' updated."))
            } else {
                let id = state
                    .db
                    .create_goal(&name, target_cents, saved.unwrap_or(0), None, target_date, monthly.unwrap_or(0))
                    .await
                    .map_err(err)?;
                Ok(format!("Goal #{id} '{name}' created (target {}).", render::gbp(target_cents)))
            }
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
