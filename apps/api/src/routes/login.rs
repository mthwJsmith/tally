//! Auth routes: setup wizard, login, 2FA enrol/verify/recovery, logout.
//!
//! Cookie-based sessions for the web UI. Cookie name = `auth::SESSION_COOKIE`.
//! No CSRF token — we rely on `SameSite=Strict` on the cookie + the absence of cookie-bearing
//! cross-site forms in our flows. Combined with Cloudflare Access at the edge.

use crate::auth::{self, LoginInput, VerifyTotpInput, SESSION_COOKIE};
use crate::ratelimit;
use crate::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use time::Duration;

fn session_cookie(token: &str, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, token.to_string()))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(secure)
        .max_age(Duration::days(30))
        .build()
}

fn clear_cookie() -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, String::new()))
        .path("/")
        .max_age(Duration::seconds(0))
        .build()
}

fn is_secure(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|p| p.eq_ignore_ascii_case("https"))
        .unwrap_or(false)
}

/// Best-effort client IP for rate-limiting keys (first hop of X-Forwarded-For).
fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "noip".to_string())
}

/// Standard 429 response body when a key is locked out.
fn too_many(wait: std::time::Duration) -> (StatusCode, String) {
    (
        StatusCode::TOO_MANY_REQUESTS,
        format!("too many attempts — retry in {}s", wait.as_secs().max(1)),
    )
}

#[derive(Debug, Deserialize)]
pub struct SetupBody {
    pub username: String,
    pub password: String,
}

/// First-run admin registration. Refuses once any user exists.
pub async fn setup(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<SetupBody>,
) -> Result<(CookieJar, Json<Value>), (StatusCode, String)> {
    let count = auth::count_users(&state.db).await.map_err(internal)?;
    if count > 0 {
        return Err((StatusCode::FORBIDDEN, "setup already complete".into()));
    }
    if body.username.trim().is_empty() || body.password.len() < 8 {
        return Err((
            StatusCode::BAD_REQUEST,
            "username required, password ≥ 8 chars".into(),
        ));
    }
    let user_id = auth::create_user(&state.db, body.username.trim(), &body.password)
        .await
        .map_err(internal)?;
    let token = auth::create_session(&state.db, user_id, false, None, None)
        .await
        .map_err(internal)?;
    let _ = auth::touch_last_login(&state.db, user_id).await;
    let jar = jar.add(session_cookie(&token, is_secure(&headers)));
    Ok((jar, Json(json!({ "ok": true, "user_id": user_id }))))
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<LoginInput>,
) -> Result<(CookieJar, Json<Value>), (StatusCode, String)> {
    let key = format!("login:{}", client_ip(&headers));
    if let Err(wait) = ratelimit::check(&key) {
        return Err(too_many(wait));
    }
    let user = match auth::find_user_by_username(&state.db, body.username.trim())
        .await
        .map_err(internal)?
    {
        Some(u) => u,
        None => {
            ratelimit::record_failure(&key);
            return Err((StatusCode::UNAUTHORIZED, "invalid credentials".into()));
        }
    };
    if !auth::verify_password(&body.password, &user.password_hash) {
        ratelimit::record_failure(&key);
        return Err((StatusCode::UNAUTHORIZED, "invalid credentials".into()));
    }
    ratelimit::record_success(&key);
    let awaiting_2fa = user.totp_enrolled == 1;
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or("").trim().to_string());
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let token = auth::create_session(
        &state.db,
        user.id,
        awaiting_2fa,
        ip.as_deref(),
        ua.as_deref(),
    )
    .await
    .map_err(internal)?;
    if !awaiting_2fa {
        let _ = auth::touch_last_login(&state.db, user.id).await;
    }
    let jar = jar.add(session_cookie(&token, is_secure(&headers)));
    Ok((
        jar,
        Json(json!({
            "ok": true,
            "requires_2fa": awaiting_2fa,
        })),
    ))
}

pub async fn verify_2fa(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<VerifyTotpInput>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let key = format!("2fa:{}", client_ip(&headers));
    if let Err(wait) = ratelimit::check(&key) {
        return Err(too_many(wait));
    }
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or((StatusCode::UNAUTHORIZED, "no session".into()))?;
    let session = auth::get_session(&state.db, &token)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid session".into()))?;
    if session.awaiting_2fa == 0 {
        return Ok(Json(json!({ "ok": true, "note": "already verified" })));
    }
    let user = sqlx::query_as::<_, auth::User>("SELECT * FROM users WHERE id = ?")
        .bind(session.user_id)
        .fetch_one(&state.db.pool)
        .await
        .map_err(internal)?;
    let secret = auth::decrypt_totp_secret(&state.db, &user).map_err(internal)?;
    if !auth::verify_totp(&secret, &body.code, &user.username) {
        ratelimit::record_failure(&key);
        return Err((StatusCode::UNAUTHORIZED, "invalid code".into()));
    }
    ratelimit::record_success(&key);
    auth::complete_2fa_session(&state.db, &token)
        .await
        .map_err(internal)?;
    let _ = auth::touch_last_login(&state.db, user.id).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn enrol_2fa(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Json<Value>, (StatusCode, String)> {
    let user = current_user(&state, &jar).await?;
    let secret = auth::fresh_totp_secret();
    let totp = auth::make_totp(&secret, &user.username).map_err(internal)?;
    let qr_png_b64 = totp.get_qr_base64().map_err(internal_str)?;
    let provisioning = totp.get_url();
    // We DO NOT save the secret yet — only after confirm with a valid code.
    let secret_b64 =
        base64::engine::general_purpose::STANDARD.encode(&secret);
    Ok(Json(json!({
        "secret_b64": secret_b64,
        "qr_png_base64": qr_png_b64,
        "provisioning_uri": provisioning,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ConfirmTotpBody {
    pub secret_b64: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct ConfirmTotpReply {
    pub ok: bool,
    pub recovery_codes: Vec<String>,
}

pub async fn confirm_2fa(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
    Json(b): Json<ConfirmTotpBody>,
) -> Result<Json<ConfirmTotpReply>, (StatusCode, String)> {
    let user = current_user(&state, &jar).await?;
    let secret = base64::engine::general_purpose::STANDARD
        .decode(&b.secret_b64)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad secret_b64: {e}")))?;
    if !auth::verify_totp(&secret, &b.code, &user.username) {
        return Err((StatusCode::UNAUTHORIZED, "code didn't match".into()));
    }
    let codes = auth::new_recovery_codes(10);
    auth::save_totp_secret(&state.db, user.id, &secret, &codes)
        .await
        .map_err(internal)?;
    Ok(Json(ConfirmTotpReply {
        ok: true,
        recovery_codes: codes,
    }))
}

#[derive(Debug, Deserialize)]
pub struct RecoveryBody {
    pub code: String,
}

pub async fn recovery(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(b): Json<RecoveryBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let key = format!("recovery:{}", client_ip(&headers));
    if let Err(wait) = ratelimit::check(&key) {
        return Err(too_many(wait));
    }
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or((StatusCode::UNAUTHORIZED, "no session".into()))?;
    let session = auth::get_session(&state.db, &token)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid session".into()))?;
    let user = sqlx::query_as::<_, auth::User>("SELECT * FROM users WHERE id = ?")
        .bind(session.user_id)
        .fetch_one(&state.db.pool)
        .await
        .map_err(internal)?;
    let ok = auth::consume_recovery_code(&state.db, &user, &b.code)
        .await
        .map_err(internal)?;
    if !ok {
        ratelimit::record_failure(&key);
        return Err((StatusCode::UNAUTHORIZED, "invalid recovery code".into()));
    }
    ratelimit::record_success(&key);
    auth::complete_2fa_session(&state.db, &token)
        .await
        .map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn logout(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<(CookieJar, Json<Value>), (StatusCode, String)> {
    if let Some(c) = jar.get(SESSION_COOKIE) {
        let _ = auth::delete_session(&state.db, c.value()).await;
    }
    Ok((jar.add(clear_cookie()), Json(json!({ "ok": true }))))
}

pub async fn me(
    State(state): State<Arc<AppState>>,
    jar: CookieJar,
) -> Result<Json<Value>, (StatusCode, String)> {
    let setup_required = auth::count_users(&state.db).await.unwrap_or(0) == 0;
    if setup_required {
        return Ok(Json(json!({
            "setup_required": true,
            "authenticated": false,
        })));
    }
    let token = match jar.get(SESSION_COOKIE) {
        Some(c) => c.value().to_string(),
        None => {
            return Ok(Json(json!({
                "authenticated": false,
                "setup_required": false,
            })));
        }
    };
    let Some(session) = auth::get_session(&state.db, &token).await.map_err(internal)? else {
        return Ok(Json(json!({
            "authenticated": false,
            "setup_required": false,
        })));
    };
    let user = sqlx::query_as::<_, auth::User>("SELECT * FROM users WHERE id = ?")
        .bind(session.user_id)
        .fetch_one(&state.db.pool)
        .await
        .map_err(internal)?;
    Ok(Json(json!({
        "authenticated": session.awaiting_2fa == 0,
        "awaiting_2fa": session.awaiting_2fa == 1,
        "totp_enrolled": user.totp_enrolled == 1,
        "username": user.username,
        "setup_required": false,
    })))
}

async fn current_user(
    state: &Arc<AppState>,
    jar: &CookieJar,
) -> Result<auth::User, (StatusCode, String)> {
    let token = jar
        .get(SESSION_COOKIE)
        .map(|c| c.value().to_string())
        .ok_or((StatusCode::UNAUTHORIZED, "no session".into()))?;
    let session = auth::get_session(&state.db, &token)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid session".into()))?;
    // A session still flagged awaiting_2fa has only passed the first factor (password).
    // It must NOT count as authenticated — otherwise a password-only attacker could call
    // /auth/2fa/enrol + /auth/2fa/confirm and silently overwrite the victim's TOTP secret,
    // defeating the second factor entirely.
    if session.awaiting_2fa == 1 {
        return Err((StatusCode::UNAUTHORIZED, "2FA required".into()));
    }
    let user = sqlx::query_as::<_, auth::User>("SELECT * FROM users WHERE id = ?")
        .bind(session.user_id)
        .fetch_one(&state.db.pool)
        .await
        .map_err(internal)?;
    Ok(user)
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
fn internal_str(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}
