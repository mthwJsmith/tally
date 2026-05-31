//! AI categorisation endpoints (OpenRouter-backed).
//!
//! POST /api/ai/transactions/:id/suggest   — get a category suggestion for one txn
//! POST /api/ai/transactions/bulk           — categorise all uncategorised in date window
//! GET  /api/ai/settings                    — current AI config (key set? model name?)
//! PUT  /api/ai/settings                    — set api_key or model

use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct SettingsBody {
    pub openrouter_api_key: Option<String>,
    pub openrouter_model: Option<String>,
    /// pass true to wipe the stored API key
    #[serde(default)]
    pub clear_key: bool,
    /// enable/disable automatic categorisation of new transactions after each sync
    pub auto_categorise: Option<bool>,
}

pub async fn get_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let configured = state.ai.is_configured().await;
    let model = state.ai.get_model().await.map_err(internal)?;
    let auto = state
        .db
        .get_setting("ai_auto_categorise")
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some("1");
    Ok(Json(json!({
        "openrouter_configured": configured,
        "openrouter_model": model,
        "auto_categorise": auto,
    })))
}

pub async fn update_settings(
    State(state): State<Arc<AppState>>,
    Json(b): Json<SettingsBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if b.clear_key {
        state.ai.clear_api_key().await.map_err(internal)?;
    } else if let Some(k) = b.openrouter_api_key.as_deref() {
        if !k.trim().is_empty() {
            state.ai.set_api_key(k.trim()).await.map_err(internal)?;
        }
    }
    if let Some(m) = b.openrouter_model.as_deref() {
        if !m.trim().is_empty() {
            state.ai.set_model(m.trim()).await.map_err(internal)?;
        }
    }
    if let Some(auto) = b.auto_categorise {
        state
            .db
            .set_setting("ai_auto_categorise", if auto { "1" } else { "0" })
            .await
            .map_err(internal)?;
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn suggest_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let txn = state
        .db
        .get_transaction(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "transaction not found".to_string()))?;
    let categories = state.db.list_categories().await.map_err(internal)?;
    let suggested = state
        .ai
        .suggest_category(&txn, &categories)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("ai: {e:#}")))?;
    Ok(Json(json!({
        "transaction_id": id,
        "suggested_category_id": suggested,
    })))
}

#[derive(Debug, Deserialize)]
pub struct BulkBody {
    /// Maximum number to process this call. Defaults to 25 to keep within rate limits.
    pub limit: Option<i64>,
    /// If true, only categorise transactions that have no category_id yet.
    #[serde(default = "default_true")]
    pub only_uncategorised: bool,
    /// If true (default), apply the suggested categories. If false, return suggestions without saving.
    #[serde(default = "default_true")]
    pub apply: bool,
}
fn default_true() -> bool {
    true
}

pub async fn bulk(
    State(state): State<Arc<AppState>>,
    Json(b): Json<BulkBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let limit = b.limit.unwrap_or(25).clamp(1, 200);
    let categories = state.db.list_categories().await.map_err(internal)?;
    let mut txns = state
        .db
        .list_transactions(None, None, None, None, None, None, None, None, limit * 4, 0)
        .await
        .map_err(internal)?;
    if b.only_uncategorised {
        txns.retain(|t| t.category_id.is_none());
    }
    txns.truncate(limit as usize);

    let mut applied = 0;
    let mut skipped = 0;
    let mut errors: Vec<String> = Vec::new();
    let mut suggestions: Vec<Value> = Vec::new();
    for t in &txns {
        match state.ai.suggest_category(t, &categories).await {
            Ok(Some(cat_id)) => {
                suggestions.push(json!({
                    "transaction_id": t.id,
                    "suggested_category_id": cat_id,
                }));
                if b.apply {
                    if let Err(e) = state
                        .db
                        .update_transaction_category(t.id, Some(cat_id))
                        .await
                    {
                        errors.push(format!("txn {}: {e:#}", t.id));
                    } else {
                        applied += 1;
                    }
                }
            }
            Ok(None) => skipped += 1,
            Err(e) => errors.push(format!("txn {}: {e:#}", t.id)),
        }
    }

    Ok(Json(json!({
        "processed": txns.len(),
        "applied": applied,
        "skipped_no_match": skipped,
        "errors": errors,
        "suggestions": suggestions,
    })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
