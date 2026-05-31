use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let bills = state.db.list_bills().await.map_err(internal)?;
    Ok(Json(json!({ "bills": bills })))
}

#[derive(Debug, Deserialize)]
pub struct UpcomingQuery {
    pub within_days: Option<i64>,
}

pub async fn upcoming(
    State(state): State<Arc<AppState>>,
    Query(q): Query<UpcomingQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let days = q.within_days.unwrap_or(30).clamp(1, 365);
    let bills = state
        .db
        .list_bills_due_within(days)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "bills": bills, "within_days": days })))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    pub expected_amount_min_cents: i64,
    pub expected_amount_max_cents: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
    pub repeat_freq: String,
    pub next_expected_date: Option<i64>,
    pub match_description_regex: Option<String>,
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
        .create_bill(
            &b.name,
            b.expected_amount_min_cents,
            b.expected_amount_max_cents,
            &b.currency,
            &b.repeat_freq,
            b.next_expected_date,
            b.match_description_regex.as_deref(),
            None,
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_bill(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
