//! Middleware: gate `/api/*` routes behind a valid (non-awaiting-2fa) session OR a valid API token.

use crate::auth::{self, SESSION_COOKIE};
use crate::AppState;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::CookieJar;
use std::sync::Arc;

pub async fn require_auth(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    // 1) API token via Authorization: Bearer <token>
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
                    let _ = sqlx::query(
                        "UPDATE api_tokens SET last_used_at = ? WHERE token_hash = ?",
                    )
                    .bind(chrono::Utc::now().timestamp())
                    .bind(&token_hash)
                    .execute(&state.db.pool)
                    .await;
                    return Ok(next.run(req).await);
                }
            }
        }
    }

    // 2) Cookie session
    let Some(c) = jar.get(SESSION_COOKIE) else {
        return Err((StatusCode::UNAUTHORIZED, "not authenticated"));
    };
    let Some(session) = auth::get_session(&state.db, c.value())
        .await
        .ok()
        .flatten()
    else {
        return Err((StatusCode::UNAUTHORIZED, "invalid session"));
    };
    if session.awaiting_2fa == 1 {
        return Err((StatusCode::UNAUTHORIZED, "awaiting 2FA"));
    }
    Ok(next.run(req).await)
}
