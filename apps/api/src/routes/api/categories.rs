use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    pub parent_id: Option<i64>,
    pub icon: Option<String>,
    pub colour: Option<String>,
}

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let cats = state.db.list_categories().await.map_err(internal)?;
    Ok(Json(json!({ "categories": cats })))
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state
        .db
        .create_category(&body.name, body.parent_id, body.icon.as_deref(), body.colour.as_deref())
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .db
        .update_category(id, &body.name, body.parent_id, body.icon.as_deref(), body.colour.as_deref())
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_category(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
