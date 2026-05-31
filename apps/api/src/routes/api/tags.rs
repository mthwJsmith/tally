use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let tags = state.db.list_tags().await.map_err(internal)?;
    Ok(Json(json!({ "tags": tags })))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state.db.upsert_tag(&body.name).await.map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
pub struct AttachBody {
    pub transaction_id: i64,
    pub tag_id: i64,
}

pub async fn attach(
    State(state): State<Arc<AppState>>,
    Json(b): Json<AttachBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.tag_transaction(b.transaction_id, b.tag_id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn detach(
    State(state): State<Arc<AppState>>,
    Path((txn_id, tag_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.untag_transaction(txn_id, tag_id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
