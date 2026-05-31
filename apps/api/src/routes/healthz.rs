use axum::Json;
use serde_json::{json, Value};

/// Liveness probe. Intentionally returns no app details — it is unauthenticated (used by the
/// container healthcheck), so it must not leak consent counts, versions, or other internals.
pub async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
