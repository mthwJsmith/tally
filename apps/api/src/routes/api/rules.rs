use crate::rules::CompiledRules;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let rules = state.db.list_rules(false).await.map_err(internal)?;
    Ok(Json(json!({ "rules": rules })))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default = "default_priority")]
    pub priority: i64,
    pub match_description_regex: Option<String>,
    pub match_merchant_regex: Option<String>,
    pub match_min_amount_cents: Option<i64>,
    pub match_max_amount_cents: Option<i64>,
    pub match_account_id: Option<i64>,
    pub match_is_credit: Option<i64>,
    pub set_category_id: Option<i64>,
    pub add_tag_ids: Option<Vec<i64>>,
    pub set_notes: Option<String>,
}
fn default_priority() -> i64 {
    100
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let tag_ids_json = b
        .add_tag_ids
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());
    let id = state
        .db
        .create_rule(
            &b.name,
            b.priority,
            b.match_description_regex.as_deref(),
            b.match_merchant_regex.as_deref(),
            b.match_min_amount_cents,
            b.match_max_amount_cents,
            b.match_account_id,
            b.match_is_credit,
            b.set_category_id,
            tag_ids_json.as_deref(),
            b.set_notes.as_deref(),
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_rule(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct ToggleBody {
    pub enabled: bool,
}

pub async fn toggle(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<ToggleBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.set_rule_enabled(id, b.enabled).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

/// Dry-run a single rule across recent transactions, return matching transaction count + sample.
pub async fn test_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let rule = state
        .db
        .get_rule(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "rule not found".to_string()))?;
    let compiled = CompiledRules::compile(vec![rule]);
    // Page through last 500 transactions for the dry-run preview.
    let txns = state
        .db
        .list_transactions(None, None, None, None, None, None, None, None, 500, 0)
        .await
        .map_err(internal)?;
    let mut matched = Vec::new();
    for t in &txns {
        let eff = crate::rules::apply(t, &compiled);
        if !eff.is_empty() {
            matched.push(json!({
                "transaction_id": t.id,
                "description": t.description,
                "amount_cents": t.amount_cents,
                "set_category_id": eff.set_category_id,
                "add_tag_ids": eff.add_tag_ids,
            }));
        }
    }
    Ok(Json(json!({
        "matched_count": matched.len(),
        "matches": matched.into_iter().take(20).collect::<Vec<_>>(),
    })))
}

/// Apply ALL enabled rules to every existing transaction. Useful after creating a new rule.
pub async fn run_all(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let rules = state.db.list_rules(true).await.map_err(internal)?;
    let compiled = CompiledRules::compile(rules);
    let (matched, mutated) = crate::rules::run_all(&state.db, &compiled, false)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "matched": matched, "mutated": mutated })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
