//! OAuth 2.0 Protected Resource Metadata (RFC 9728) for the MCP endpoint.
//!
//! tally is a *resource server* only (see `oidc.rs` and `docs/auth-refactor.md`): it no longer
//! runs an authorization server. This document points MCP clients at the external IdP
//! (Authentik/Keycloak) that issues access tokens. When `TALLY_OIDC_ISSUER` is unset,
//! `authorization_servers` is empty and `/mcp` accepts only legacy API tokens.

use crate::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::Json;
use serde_json::{json, Value};
use std::sync::Arc;

/// Public origin, derived from the request Host so discovery URLs match what the client used.
fn base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("localhost:3001");
    let scheme = if host.starts_with("localhost") || host.starts_with("127.") {
        "http"
    } else {
        "https"
    };
    format!("{scheme}://{host}")
}

pub async fn protected_resource_metadata(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Json<Value> {
    let base = base_url(&headers);
    let authorization_servers = state
        .oidc
        .as_ref()
        .map(|c| vec![c.issuer.clone()])
        .unwrap_or_default();
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": authorization_servers,
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["read", "write"]
    }))
}
