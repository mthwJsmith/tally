use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let accounts = state
        .db
        .list_all_enabled_accounts()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    Ok(Json(json!({ "accounts": accounts })))
}

#[derive(Deserialize)]
pub struct AccountPatch {
    pub custom_display_name: Option<String>,
}

/// PATCH /api/accounts/:id — for now only `custom_display_name` is mutable. Pass empty string
/// or null to clear the override and fall back to TrueLayer's `display_name`.
pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<AccountPatch>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let custom = body
        .custom_display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    state
        .db
        .set_account_custom_display_name(id, custom)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")))?;
    Ok(Json(json!({ "ok": true })))
}
