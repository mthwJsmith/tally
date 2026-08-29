//! Middleware: gate protected routes behind a valid axum-login session (fully authenticated,
//! i.e. past 2FA) OR a valid OIDC access token issued by the configured IdP.
//!
//! There is no legacy hashed-API-token path. Programmatic clients (Home Assistant, scripts)
//! authenticate with an OIDC bearer token from the same provider that powers `/mcp`.

use crate::auth_backend::AuthSession;
use crate::oidc;
use crate::AppState;
use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    auth_session: AuthSession,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    // 1) Web-UI session (axum-login) — present only after a full, post-2FA login.
    if auth_session.user.is_some() {
        return Ok(next.run(req).await);
    }
    // 2) OIDC access token from the configured IdP, for programmatic clients.
    if let Some(cfg) = &state.oidc {
        if let Some(raw) = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
        {
            if let Some(claims) = oidc::validate(cfg, raw.trim()).await {
                // Same scope model as /mcp: any valid token may read, but mutating requests
                // need the `write` scope (or the TALLY_MCP_WRITE self-host escape hatch).
                // Without this a "read-only" token could bypass the MCP gate by calling the
                // REST mutations directly.
                let is_read = matches!(*req.method(), Method::GET | Method::HEAD);
                if is_read || claims.has_scope("write") || oidc::write_allowed_by_env() {
                    return Ok(next.run(req).await);
                }
                return Err((StatusCode::FORBIDDEN, "token lacks write scope"));
            }
        }
    }
    Err((StatusCode::UNAUTHORIZED, "not authenticated"))
}
