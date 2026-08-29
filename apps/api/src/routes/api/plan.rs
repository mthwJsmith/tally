//! CRUD for the planning layer: plan_accounts (manual accounts + planning metadata on synced
//! ones) and plan_events (the dated cashflows/transfers on the Ahead forecast). All amounts are
//! in pence/cents — the frontend speaks cents; the MCP layer converts pounds → cents for the LLM.

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
fn not_found(what: &str) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("{what} not found"))
}
/// Map an optional string to None when blank, so the UI can clear a nullable field with "".
fn blank_none(s: Option<String>) -> Option<String> {
    s.and_then(|v| if v.trim().is_empty() { None } else { Some(v) })
}

// ---- plan accounts --------------------------------------------------------

pub async fn list_accounts(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.seed_plan_accounts_from_synced().await.map_err(internal)?;
    let accounts = state.db.list_plan_accounts().await.map_err(internal)?;
    Ok(Json(json!({ "accounts": accounts })))
}

pub async fn sync_accounts(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.seed_plan_accounts_from_synced().await.map_err(internal)?;
    let accounts = state.db.list_plan_accounts().await.map_err(internal)?;
    Ok(Json(json!({ "accounts": accounts })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAccountBody {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub balance_cents: Option<i64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub floor_cents: Option<i64>,
    #[serde(default)]
    pub credit_limit_cents: Option<i64>,
    #[serde(default)]
    pub apr_bps: Option<i64>,
    #[serde(default)]
    pub statement_day: Option<i64>,
}

pub async fn create_account(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateAccountBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // Manual accounts only — synced ones are seeded automatically from linked bank accounts.
    let id = state
        .db
        .create_plan_account(
            &b.name,
            b.kind.as_deref().unwrap_or("current"),
            "manual",
            None,
            b.balance_cents.unwrap_or(0),
            b.currency.as_deref().unwrap_or("GBP"),
            b.floor_cents.unwrap_or(0),
            b.credit_limit_cents,
            b.apr_bps,
            b.statement_day,
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchAccountBody {
    pub name: Option<String>,
    pub kind: Option<String>,
    pub balance_cents: Option<i64>,
    pub currency: Option<String>,
    pub floor_cents: Option<i64>,
    pub cliff_date_iso: Option<String>,
    pub cliff_new_floor_cents: Option<i64>,
    pub credit_limit_cents: Option<i64>,
    pub apr_bps: Option<i64>,
    pub statement_day: Option<i64>,
    pub payment_intent: Option<String>,
    pub sort_order: Option<i64>,
    pub enabled: Option<i64>,
}

pub async fn patch_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<PatchAccountBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cur = state.db.get_plan_account(id).await.map_err(internal)?.ok_or(not_found("account"))?;
    // synced accounts keep their live balance; only manual balances are user-settable here.
    let balance = if cur.source == "manual" {
        b.balance_cents.unwrap_or(cur.balance_cents)
    } else {
        cur.balance_cents
    };
    state
        .db
        .update_plan_account(
            id,
            b.name.as_deref().unwrap_or(&cur.name),
            b.kind.as_deref().unwrap_or(&cur.kind),
            balance,
            b.currency.as_deref().unwrap_or(&cur.currency),
            b.floor_cents.unwrap_or(cur.floor_cents),
            blank_none(b.cliff_date_iso).or(cur.cliff_date).as_deref(),
            b.cliff_new_floor_cents.or(cur.cliff_new_floor_cents),
            b.credit_limit_cents.or(cur.credit_limit_cents),
            b.apr_bps.or(cur.apr_bps),
            b.statement_day.or(cur.statement_day),
            blank_none(b.payment_intent).or(cur.payment_intent).as_deref(),
            b.sort_order.unwrap_or(cur.sort_order),
            b.enabled.unwrap_or(cur.enabled),
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_plan_account(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

// ---- action plan (the "moves to make" checklist) --------------------------

const ACTION_KEY: &str = "ahead_action_plan";

pub async fn get_actions(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let text = state.db.get_setting(ACTION_KEY).await.map_err(internal)?.unwrap_or_default();
    Ok(Json(json!({ "text": text })))
}

#[derive(Debug, Deserialize)]
pub struct ActionsBody {
    pub text: String,
}

pub async fn put_actions(
    State(state): State<Arc<AppState>>,
    Json(b): Json<ActionsBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.set_setting(ACTION_KEY, &b.text).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

// ---- plan events ----------------------------------------------------------

pub async fn list_events(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let events = state.db.list_plan_events().await.map_err(internal)?;
    Ok(Json(json!({ "events": events })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEventBody {
    pub date_iso: String,
    pub label: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub account_id: Option<i64>,
    #[serde(default)]
    pub to_account_id: Option<i64>,
    pub amount_cents: i64,
    #[serde(default)]
    pub recurrence: Option<String>,
    #[serde(default)]
    pub recur_until_iso: Option<String>,
    #[serde(default)]
    pub category_id: Option<i64>,
    #[serde(default)]
    pub match_regex: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

pub async fn create_event(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateEventBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state
        .db
        .create_plan_event(
            &b.date_iso,
            &b.label,
            b.source.as_deref().unwrap_or("planned"),
            b.account_id,
            b.to_account_id,
            b.amount_cents,
            b.recurrence.as_deref().unwrap_or("none"),
            blank_none(b.recur_until_iso).as_deref(),
            b.category_id,
            blank_none(b.match_regex).as_deref(),
            blank_none(b.note).as_deref(),
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchEventBody {
    pub date_iso: Option<String>,
    pub label: Option<String>,
    pub source: Option<String>,
    pub account_id: Option<i64>,
    pub to_account_id: Option<i64>,
    pub amount_cents: Option<i64>,
    pub recurrence: Option<String>,
    pub recur_until_iso: Option<String>,
    pub category_id: Option<i64>,
    pub match_regex: Option<String>,
    pub note: Option<String>,
    pub enabled: Option<i64>,
    /// Set true to clear to_account_id (turn a transfer into a one-sided event).
    #[serde(default)]
    pub clear_to_account: bool,
}

pub async fn patch_event(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<PatchEventBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cur = state.db.get_plan_event(id).await.map_err(internal)?.ok_or(not_found("event"))?;
    let to_account = if b.clear_to_account { None } else { b.to_account_id.or(cur.to_account_id) };
    state
        .db
        .update_plan_event(
            id,
            b.date_iso.as_deref().unwrap_or(&cur.date),
            b.label.as_deref().unwrap_or(&cur.label),
            b.source.as_deref().unwrap_or(&cur.source),
            b.account_id.or(cur.account_id),
            to_account,
            b.amount_cents.unwrap_or(cur.amount_cents),
            b.recurrence.as_deref().unwrap_or(&cur.recurrence),
            blank_none(b.recur_until_iso).or(cur.recur_until).as_deref(),
            b.category_id.or(cur.category_id),
            blank_none(b.match_regex).or(cur.match_regex).as_deref(),
            blank_none(b.note).or(cur.note).as_deref(),
            b.enabled.unwrap_or(cur.enabled),
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_event(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_plan_event(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}
