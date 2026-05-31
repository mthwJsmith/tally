use crate::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(State(state): State<Arc<AppState>>) -> Result<Json<Value>, (StatusCode, String)> {
    let items = state.db.list_watchlist().await.map_err(internal)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let sources = state.db.watchlist_sources(item.id).await.map_err(internal)?;
        out.push(json!({ "item": item, "sources": sources }));
    }
    let deals = state.db.recent_deals(50).await.map_err(internal)?;
    Ok(Json(json!({ "items": out, "deals": deals })))
}

#[derive(Debug, Deserialize)]
pub struct SourceInput {
    pub kind: String, // 'rss' | 'changedetection'
    pub r#ref: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateBody {
    pub name: String,
    pub keywords: Option<String>,
    pub target_price_cents: Option<i64>,
    #[serde(default = "gbp")]
    pub currency: String,
    #[serde(default)]
    pub sources: Vec<SourceInput>,
}
fn gbp() -> String {
    "GBP".to_string()
}

/// Build a HotUKDeals keyword-search RSS feed for free-text terms.
pub fn hukd_feed(query: &str) -> String {
    let q = serde_urlencoded::to_string([("q", query.trim())]).unwrap_or_default();
    format!("https://www.hotukdeals.com/search.rss?{q}")
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(b): Json<CreateBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let id = state
        .db
        .create_watchlist_item(
            b.name.trim(),
            b.keywords.as_deref(),
            b.target_price_cents,
            &b.currency,
        )
        .await
        .map_err(internal)?;

    let mut sources = b.sources;
    // Smart default: if you didn't paste any feeds, auto-watch HotUKDeals for the name
    // (plus keywords). You type a product, we build the search feed.
    if sources.is_empty() {
        let q = match b.keywords.as_deref() {
            Some(k) if !k.trim().is_empty() => format!("{} {}", b.name.trim(), k.trim()),
            _ => b.name.trim().to_string(),
        };
        sources.push(SourceInput {
            kind: "rss".to_string(),
            r#ref: hukd_feed(&q),
        });
    }
    for s in sources {
        state
            .db
            .add_watchlist_source(id, s.kind.trim(), s.r#ref.trim())
            .await
            .map_err(internal)?;
    }
    Ok(Json(json!({ "id": id })))
}

pub async fn add_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(b): Json<SourceInput>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let sid = state
        .db
        .add_watchlist_source(id, b.kind.trim(), b.r#ref.trim())
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "id": sid })))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.delete_watchlist_item(id).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct DealsQuery {
    pub limit: Option<i64>,
}

pub async fn deals(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DealsQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let deals = state
        .db
        .recent_deals(q.limit.unwrap_or(50))
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "deals": deals })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
