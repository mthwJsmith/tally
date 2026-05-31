use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let budgets = state.db.list_budgets().await.map_err(internal)?;
    Ok(Json(json!({ "budgets": budgets })))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    pub category_id: Option<i64>,
    pub amount_cents: i64,
    pub period: String,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub rollover: bool,
}
fn default_currency() -> String {
    "GBP".to_string()
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state
        .db
        .create_budget(&b.name, b.category_id, b.amount_cents, &b.period, &b.currency, b.rollover)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_budget(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let b = state
        .db
        .get_budget(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "budget not found".to_string()))?;
    let spent = state.db.budget_period_spend(&b).await.map_err(internal)?;
    let pct = if b.amount_cents > 0 {
        (spent as f64 / b.amount_cents as f64) * 100.0
    } else {
        0.0
    };
    Ok(Json(json!({
        "budget": b,
        "spent_cents": spent,
        "percent": pct,
        "remaining_cents": (b.amount_cents - spent).max(0),
        "over_budget": spent > b.amount_cents,
    })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
