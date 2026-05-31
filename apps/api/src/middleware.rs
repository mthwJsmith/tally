//! Middleware: gate protected routes behind a valid axum-login session (fully authenticated,
//! i.e. past 2FA) OR a valid `Authorization: Bearer <api_token>`.

use crate::auth;
use crate::auth_backend::AuthSession;
use crate::AppState;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    auth_session: AuthSession,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    // 1) API token via Authorization: Bearer <token> (for scripts / Home Assistant).
    if let Some(h) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(s) = h.to_str() {
            if let Some(rest) = s.strip_prefix("Bearer ") {
                let token_hash = auth::hash_api_token(rest.trim());
                let row: Result<Option<(i64,)>, _> = sqlx::query_as(
                    "SELECT user_id FROM api_tokens WHERE token_hash = ? AND revoked_at IS NULL",
                )
                .bind(&token_hash)
                .fetch_optional(&state.db.pool)
                .await;
                if let Ok(Some(_)) = row {
                    // Best-effort last_used_at bump.
                    let _ =
                        sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE token_hash = ?")
                            .bind(chrono::Utc::now().timestamp())
                            .bind(&token_hash)
                            .execute(&state.db.pool)
                            .await;
                    return Ok(next.run(req).await);
                }
            }
        }
    }

    // 2) axum-login session — populated only after a full (post-2FA) login.
    if auth_session.user.is_some() {
        return Ok(next.run(req).await);
    }
    Err((StatusCode::UNAUTHORIZED, "not authenticated"))
}
