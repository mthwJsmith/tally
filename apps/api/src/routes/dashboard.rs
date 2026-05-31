use crate::models::{Consent, SyncLogEntry};
use crate::AppState;
use askama::Template;
use axum::extract::State;
use axum::response::Html;
use std::sync::Arc;

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate {
    pub consents: Vec<Consent>,
    pub recent_logs: Vec<SyncLogEntry>,
}

pub async fn index(State(state): State<Arc<AppState>>) -> Html<String> {
    let consents = state.db.list_consents().await.unwrap_or_default();
    let recent_logs = state.db.recent_sync_logs(20).await.unwrap_or_default();
    let tmpl = IndexTemplate {
        consents,
        recent_logs,
    };
    Html(tmpl.render().unwrap_or_else(|e| format!("template error: {e}")))
}
