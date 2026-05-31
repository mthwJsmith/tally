use crate::AppState;
use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn healthz(State(state): State<Arc<AppState>>) -> Json<Value> {
    let consent_count = state
        .db
        .list_consents()
        .await
        .map(|v| v.len())
        .unwrap_or(0);
    Json(json!({
        "status": "ok",
        "consents": consent_count,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}
