use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let reminders = state.db.list_reminders().await.map_err(internal)?;
    Ok(Json(json!({ "reminders": reminders })))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub title: String,
    pub notes: Option<String>,
    pub freq: String, // 'hours' | 'days' | 'weeks' | 'months'
    #[serde(default = "one")]
    pub every_n: i64,
    pub anchor_day: Option<i64>,
    pub due_at: i64,
    #[serde(default)]
    pub notify_before: i64,
    #[serde(default = "yes")]
    pub notify_enabled: bool,
}
fn one() -> i64 {
    1
}
fn yes() -> bool {
    true
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state
        .db
        .create_reminder(
            b.title.trim(),
            b.notes.as_deref(),
            &b.freq,
            b.every_n.max(1),
            b.anchor_day,
            b.due_at,
            b.notify_before.max(0),
            b.notify_enabled,
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Debug, Deserialize)]
pub struct UpdateBody {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub notify_before: Option<i64>,
    pub notify_enabled: Option<bool>,
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<UpdateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .db
        .update_reminder(
            id,
            b.title.as_deref(),
            b.notes.as_deref(),
            b.notify_before,
            b.notify_enabled,
        )
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn tick(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.tick_reminder(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn untick(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.untick_reminder(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_reminder(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
