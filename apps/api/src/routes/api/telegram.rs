use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn get(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let (token, chat) = state.db.get_telegram_config().await.map_err(internal)?;
    Ok(Json(json!({
        "configured": token.is_some() && chat.is_some(),
        "chat_id": chat,
        // the bot token is never returned
    })))
}

#[derive(Debug, Deserialize)]
pub struct PutBody {
    #[serde(default)]
    pub bot_token: String, // blank = keep existing
    #[serde(default)]
    pub chat_id: String,
}

pub async fn put(
    State(state): State<Arc<AppState>>,
    Json(b): Json<PutBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .db
        .set_telegram_config(&b.bot_token, &b.chat_id)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn test(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .notifier
        .send_telegram_text("✅ tally Telegram test — it works.", false)
        .await;
    Ok(Json(json!({ "ok": true })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
