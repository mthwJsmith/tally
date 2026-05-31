use crate::importer::Importer;
use crate::AppState;
use askama::Template;
use askama_axum::IntoResponse;
use axum::extract::{Form, Path, State};
use axum::response::Redirect;
use axum::Json;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Template)]
#[template(path = "consents.html")]
pub struct ConsentsTemplate {
    pub consents: Vec<crate::models::Consent>,
}

pub async fn list(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let consents = state.db.list_consents().await.unwrap_or_default();
    ConsentsTemplate { consents }
}

#[derive(Deserialize)]
pub struct CreateForm {
    pub nickname: String,
}

/// POST /consents — kicks off OAuth for a new bank.
/// Generates a `state` token, saves the consent nickname against it, redirects user to TL.
pub async fn create(
    State(state): State<Arc<AppState>>,
    Form(form): Form<CreateForm>,
) -> Redirect {
    let nickname = form
        .nickname
        .trim()
        .to_lowercase()
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_', "-");
    if nickname.is_empty() {
        return Redirect::to("/consents?error=empty_nickname");
    }
    let token: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(40)
        .map(char::from)
        .collect();

    if let Err(e) = state.db.save_oauth_state(&token, &nickname).await {
        tracing::error!("save_oauth_state failed: {e:#}");
        return Redirect::to("/consents?error=db");
    }

    let url = state.tl.build_authorize_url(&token);
    Redirect::to(&url)
}

pub async fn sync_one(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Redirect {
    let consent = match state.db.get_consent(id).await {
        Ok(Some(c)) => c,
        _ => return Redirect::to("/?error=consent_not_found"),
    };
    let importer = Importer {
        db: state.db.clone(),
        tl: state.tl.clone(),
        notifier: state.notifier.clone(),
        ai: state.ai.clone(),
    };
    // Fire-and-forget — return to dashboard immediately, sync runs in background.
    tokio::spawn(async move {
        if let Err(e) = importer.sync_consent(&consent).await {
            tracing::error!("manual sync failed: {e:#}");
        }
    });
    Redirect::to("/?ok=sync_triggered")
}

/// POST /api/sync — trigger a background sync of every enabled consent. Returns immediately
/// with how many syncs were kicked off; the dashboard then refetches once they land.
pub async fn sync_all(State(state): State<Arc<AppState>>) -> Json<Value> {
    let consents = state.db.list_consents().await.unwrap_or_default();
    let mut triggered = 0;
    for consent in consents.into_iter().filter(|c| c.enabled == 1) {
        let importer = Importer {
            db: state.db.clone(),
            tl: state.tl.clone(),
            notifier: state.notifier.clone(),
            ai: state.ai.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = importer.sync_consent(&consent).await {
                tracing::error!("manual sync-all failed for {}: {e:#}", consent.nickname);
            }
        });
        triggered += 1;
    }
    Json(json!({ "ok": true, "triggered": triggered }))
}

pub async fn disable(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Redirect {
    let _ = state.db.set_consent_enabled(id, false).await;
    Redirect::to("/")
}

pub async fn enable(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Redirect {
    let _ = state.db.set_consent_enabled(id, true).await;
    Redirect::to("/")
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Redirect {
    let _ = state.db.delete_consent(id).await;
    Redirect::to("/")
}

#[derive(Deserialize)]
pub struct RenameBody {
    pub nickname: String,
}

/// PATCH /api/consents/:id — rename the user-visible label.
pub async fn rename(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<RenameBody>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    state
        .db
        .rename_consent(id, &body.nickname)
        .await
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, format!("{e}")))?;
    Ok(Json(json!({ "ok": true })))
}

/// Read-only JSON endpoint for HA / MCP integration.
pub async fn api_recent_transactions(State(state): State<Arc<AppState>>) -> Json<Value> {
    // Aggregate latest sync_log + per-consent state. Lightweight.
    let consents = state.db.list_consents().await.unwrap_or_default();
    let logs = state.db.recent_sync_logs(50).await.unwrap_or_default();
    Json(json!({
        "consents": consents,
        "recent_sync_log": logs,
    }))
}
