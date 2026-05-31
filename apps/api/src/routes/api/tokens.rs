//! API token management — mint / list / revoke Bearer tokens.
//!
//! These tokens authenticate non-browser clients (Home Assistant REST sensors, the MCP
//! connector, Claude, scripts) against `/api/*` and the MCP endpoint. We store only a
//! SHA-256 hash; the raw token is shown exactly once at creation.

use crate::auth;
use crate::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

async fn user_id(state: &AppState) -> Result<i64, (StatusCode, String)> {
    state
        .db
        .primary_user_id()
        .await
        .map_err(internal)?
        .ok_or((StatusCode::BAD_REQUEST, "no user configured".to_string()))
}

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = user_id(&state).await?;
    let tokens = state.db.list_api_tokens(uid).await.map_err(internal)?;
    Ok(Json(json!({ "tokens": tokens })))
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    #[serde(default = "default_scopes")]
    pub scopes: String,
}
fn default_scopes() -> String {
    "read".to_string()
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = user_id(&state).await?;
    let name = b.name.trim();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name required".to_string()));
    }
    // tally_<40 alnum>. The raw token is returned now and never again.
    let raw: String = format!(
        "tally_{}",
        rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(40)
            .map(char::from)
            .collect::<String>()
    );
    let hash = auth::hash_api_token(&raw);
    let id = state
        .db
        .create_api_token(uid, name, &hash, &b.scopes)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": id, "token": raw, "name": name, "scopes": b.scopes })))
}

pub async fn revoke(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let uid = user_id(&state).await?;
    state.db.revoke_api_token(uid, id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
