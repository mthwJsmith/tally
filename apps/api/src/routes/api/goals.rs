//! CRUD for savings goals (the Ahead rail). Amounts in pence/cents.

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
fn blank_none(s: Option<String>) -> Option<String> {
    s.and_then(|v| if v.trim().is_empty() { None } else { Some(v) })
}

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let goals = state.db.list_goals().await.map_err(internal)?;
    Ok(Json(json!({ "goals": goals })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGoalBody {
    pub name: String,
    pub target_cents: i64,
    #[serde(default)]
    pub saved_cents: Option<i64>,
    #[serde(default)]
    pub source_account_id: Option<i64>,
    #[serde(default)]
    pub target_date_iso: Option<String>,
    #[serde(default)]
    pub monthly_cents: Option<i64>,
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateGoalBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state
        .db
        .create_goal(
            &b.name,
            b.target_cents,
            b.saved_cents.unwrap_or(0),
            b.source_account_id,
            blank_none(b.target_date_iso).as_deref(),
            b.monthly_cents.unwrap_or(0),
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchGoalBody {
    pub name: Option<String>,
    pub target_cents: Option<i64>,
    pub saved_cents: Option<i64>,
    pub source_account_id: Option<i64>,
    pub target_date_iso: Option<String>,
    pub monthly_cents: Option<i64>,
    pub enabled: Option<i64>,
}

pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<PatchGoalBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cur = state
        .db
        .get_goal(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "goal not found".to_string()))?;
    state
        .db
        .update_goal(
            id,
            b.name.as_deref().unwrap_or(&cur.name),
            b.target_cents.unwrap_or(cur.target_cents),
            b.saved_cents.unwrap_or(cur.saved_cents),
            b.source_account_id.or(cur.source_account_id),
            blank_none(b.target_date_iso).or(cur.target_date).as_deref(),
            b.monthly_cents.unwrap_or(cur.monthly_cents),
            b.enabled.unwrap_or(cur.enabled),
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_goal(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}
