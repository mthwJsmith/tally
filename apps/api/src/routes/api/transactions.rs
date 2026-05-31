//! GET /api/transactions — filter / search / paginate
//! GET /api/transactions/:id
//! PATCH /api/transactions/:id — update category, notes, tags

use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub account_ids: Option<String>,   // comma-separated
    pub category_ids: Option<String>,  // comma-separated, "null" means uncategorised
    pub from: Option<i64>,             // unix seconds
    pub to: Option<i64>,
    pub min_amount_cents: Option<i64>,
    pub max_amount_cents: Option<i64>,
    pub is_credit: Option<bool>,
    pub q: Option<String>, // free-text on description + merchant
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let account_ids: Option<Vec<i64>> = q.account_ids.as_deref().map(parse_id_list);
    let category_ids: Option<Vec<i64>> = q.category_ids.as_deref().map(parse_id_list);
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);

    let txns = state
        .db
        .list_transactions(
            account_ids.as_deref(),
            category_ids.as_deref(),
            q.from,
            q.to,
            q.min_amount_cents,
            q.max_amount_cents,
            q.is_credit,
            q.q.as_deref(),
            limit,
            offset,
        )
        .await
        .map_err(internal)?;

    let total = state
        .db
        .count_transactions_filtered(
            account_ids.as_deref(),
            category_ids.as_deref(),
            q.from,
            q.to,
        )
        .await
        .unwrap_or(0);

    Ok(Json(json!({
        "transactions": txns,
        "total": total,
        "limit": limit,
        "offset": offset,
    })))
}

pub async fn get_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let txn = state
        .db
        .get_transaction(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "transaction not found".to_string()))?;
    let tags = state
        .db
        .tags_for_transaction(id)
        .await
        .unwrap_or_default();
    Ok(Json(json!({ "transaction": txn, "tags": tags })))
}

#[derive(Debug, Deserialize)]
pub struct PatchBody {
    pub category_id: Option<Option<i64>>, // explicit None to clear, omitted to leave alone
    pub notes: Option<Option<String>>,
}

pub async fn patch(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<PatchBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if let Some(cat) = body.category_id {
        state
            .db
            .update_transaction_category(id, cat)
            .await
            .map_err(internal)?;
    }
    if let Some(notes) = body.notes {
        state
            .db
            .update_transaction_notes(id, notes.as_deref())
            .await
            .map_err(internal)?;
    }
    let txn = state
        .db
        .get_transaction(id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "transaction not found".to_string()))?;
    Ok(Json(json!({ "transaction": txn })))
}

pub async fn spending_summary(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let from = q.from.unwrap_or_else(|| chrono::Utc::now().timestamp() - 30 * 86_400);
    let to = q.to.unwrap_or_else(|| chrono::Utc::now().timestamp());
    let rows = state
        .db
        .spending_by_category(from, to)
        .await
        .map_err(internal)?;
    Ok(Json(json!({
        "from": from,
        "to": to,
        "by_category": rows.into_iter().map(|(cid, cents)| json!({
            "category_id": cid,
            "amount_cents": cents,
        })).collect::<Vec<_>>(),
    })))
}

fn parse_id_list(s: &str) -> Vec<i64> {
    s.split(',').filter_map(|p| p.trim().parse::<i64>().ok()).collect()
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
