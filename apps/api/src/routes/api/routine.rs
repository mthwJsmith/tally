//! Claude routine integration. Stores the per-routine API endpoint + bearer token
//! (the token encrypted at rest) and can fire the routine via an HTTP POST, the same
//! way claude.ai's "API trigger" expects.

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn get(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let (endpoint, token) = state.db.get_routine_config().await.map_err(internal)?;
    Ok(Json(json!({
        "configured": endpoint.is_some() && token.is_some(),
        "endpoint": endpoint,
        // the bearer token is never returned
    })))
}

#[derive(Debug, Deserialize)]
pub struct PutBody {
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub token: String, // blank = keep existing
}

pub async fn put(
    State(state): State<Arc<AppState>>,
    Json(b): Json<PutBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    // The bearer token goes wherever this URL points; HTTPS only so it can't
    // be redirected onto plaintext.
    let endpoint = b.endpoint.trim();
    if !endpoint.is_empty() && !endpoint.starts_with("https://") {
        return Err((
            StatusCode::BAD_REQUEST,
            "Routine endpoint must be an https:// URL.".into(),
        ));
    }
    state
        .db
        .set_routine_config(&b.endpoint, &b.token)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

/// POST to the routine's endpoint with its bearer token to start a run.
pub async fn fire(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (endpoint, token) = state.db.get_routine_config().await.map_err(internal)?;
    let endpoint = endpoint.ok_or((
        StatusCode::BAD_REQUEST,
        "No routine endpoint saved yet.".into(),
    ))?;
    let token = token.ok_or((
        StatusCode::BAD_REQUEST,
        "No routine token saved yet.".into(),
    ))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(internal)?;
    let resp = client
        .post(&endpoint)
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Request failed: {e}")))?;

    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("Routine returned {status}: {body}"),
        ));
    }
    Ok(Json(json!({ "ok": true, "result": body })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
