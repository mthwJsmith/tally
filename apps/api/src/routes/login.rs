//! Auth routes: setup wizard, login, 2FA enrol/verify/recovery, logout.
//!
//! Sessions are owned by axum-login + tower-sessions (see `auth_backend`); we no longer
//! hand-roll a sessions table or cookie. Credentials are verified with Argon2id and TOTP
//! 2FA is a partial-auth step: on first-factor success for a 2FA-enrolled user we stash the
//! user id in the tower-session under `PENDING_2FA_KEY` and only call `AuthSession::login`
//! once the TOTP (or a recovery code) checks out.

use crate::auth;
use crate::auth_backend::{AuthSession, Credentials};
use crate::ratelimit;
use crate::AppState;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use std::net::SocketAddr;
use axum::Json;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tower_sessions::Session;

/// tower-session key holding the user id of a first-factor-authenticated, awaiting-2FA login.
const PENDING_2FA_KEY: &str = "pending_2fa_uid";

/// Whether a trusted reverse proxy sits in front (TALLY_TRUST_PROXY=true). Only then is
/// X-Forwarded-For honoured; otherwise a direct caller could mint a fresh rate-limit
/// bucket per request by spoofing the header.
static TRUST_PROXY: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
    std::env::var("TALLY_TRUST_PROXY")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
});

/// Client IP for rate-limiting keys: the TCP peer address, unless a trusted proxy is
/// declared — then the last X-Forwarded-For entry (the hop the proxy itself appended;
/// earlier entries are client-supplied and forgeable).
fn client_ip(addr: &SocketAddr, headers: &HeaderMap) -> String {
    if *TRUST_PROXY {
        if let Some(ip) = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(',').next_back())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return ip.to_string();
        }
    }
    addr.ip().to_string()
}

/// Standard 429 response when a rate-limit key is locked out.
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

async fn load_user(state: &Arc<AppState>, id: i64) -> Result<auth::User, (StatusCode, String)> {
    auth::find_user_by_id(&state.db, id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::UNAUTHORIZED, "user not found".into()))
}

/// First-run admin registration. Refuses once any user exists.
pub async fn setup(
    State(state): State<Arc<AppState>>,
    mut auth_session: AuthSession,
    Json(body): Json<SetupBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let count = auth::count_users(&state.db).await.map_err(internal)?;
    if count > 0 {
        return Err((StatusCode::FORBIDDEN, "setup already complete".into()));
    }
    if body.username.trim().is_empty() || body.password.len() < 12 {
        return Err((
            StatusCode::BAD_REQUEST,
            "username required, password ≥ 12 chars".into(),
        ));
    }
    let user_id = auth::create_user(&state.db, body.username.trim(), &body.password)
        .await
        .map_err(internal)?;
    let user = load_user(&state, user_id).await?;
    // First admin has no 2FA yet → establish a full session immediately.
    auth_session.login(&user).await.map_err(internal)?;
    let _ = auth::touch_last_login(&state.db, user_id).await;
    Ok(Json(json!({ "ok": true, "user_id": user_id })))
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    mut auth_session: AuthSession,
    Json(body): Json<auth::LoginInput>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let key = format!("login:{}", client_ip(&addr, &headers));
    if let Err(wait) = ratelimit::check(&key) {
        return Err(too_many(wait));
    }
    let creds = Credentials {
        username: body.username.clone(),
        password: body.password.clone(),
    };
    let user = match auth_session.authenticate(creds).await {
        Ok(Some(u)) => u,
        Ok(None) => {
            ratelimit::record_failure(&key);
            return Err((StatusCode::UNAUTHORIZED, "invalid credentials".into()));
        }
        Err(e) => return Err(internal(e)),
    };
    ratelimit::record_success(&key);

    if user.totp_enrolled == 1 {
        // First factor OK but 2FA required: stash a pending id; do NOT log in yet.
        session
            .insert(PENDING_2FA_KEY, user.id)
            .await
            .map_err(internal)?;
        return Ok(Json(json!({ "ok": true, "requires_2fa": true })));
    }
    auth_session.login(&user).await.map_err(internal)?;
    let _ = auth::touch_last_login(&state.db, user.id).await;
    Ok(Json(json!({ "ok": true, "requires_2fa": false })))
}

pub async fn verify_2fa(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    mut auth_session: AuthSession,
    Json(body): Json<auth::VerifyTotpInput>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let key = format!("2fa:{}", client_ip(&addr, &headers));
    if let Err(wait) = ratelimit::check(&key) {
        return Err(too_many(wait));
    }
    let Some(uid) = session
        .get::<i64>(PENDING_2FA_KEY)
        .await
        .map_err(internal)?
    else {
        return Err((StatusCode::UNAUTHORIZED, "no pending 2FA".into()));
    };
    let user = load_user(&state, uid).await?;
    let ok = auth::verify_totp_fresh(&state.db, &user, &body.code)
        .await
        .map_err(internal)?;
    if !ok {
        ratelimit::record_failure(&key);
        return Err((StatusCode::UNAUTHORIZED, "invalid code".into()));
    }
    ratelimit::record_success(&key);
    let _ = session.remove::<i64>(PENDING_2FA_KEY).await;
    auth_session.login(&user).await.map_err(internal)?;
    let _ = auth::touch_last_login(&state.db, user.id).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn enrol_2fa(auth_session: AuthSession) -> Result<Json<Value>, (StatusCode, String)> {
    // Behind require_auth → a full (post-2FA) session is guaranteed.
    let user = auth_session
        .user
        .ok_or((StatusCode::UNAUTHORIZED, "not authenticated".into()))?;
    let secret = auth::fresh_totp_secret();
    let totp = auth::make_totp(&secret, &user.username).map_err(internal)?;
    let qr_png_b64 = totp.get_qr_base64().map_err(internal_str)?;
    let provisioning = totp.get_url();
    // The secret is NOT saved until confirm with a valid code.
    let secret_b64 = base64::engine::general_purpose::STANDARD.encode(&secret);
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
    auth_session: AuthSession,
    Json(b): Json<ConfirmTotpBody>,
) -> Result<Json<ConfirmTotpReply>, (StatusCode, String)> {
    let user = auth_session
        .user
        .ok_or((StatusCode::UNAUTHORIZED, "not authenticated".into()))?;
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
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    session: Session,
    mut auth_session: AuthSession,
    Json(b): Json<RecoveryBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let key = format!("recovery:{}", client_ip(&addr, &headers));
    if let Err(wait) = ratelimit::check(&key) {
        return Err(too_many(wait));
    }
    let Some(uid) = session
        .get::<i64>(PENDING_2FA_KEY)
        .await
        .map_err(internal)?
    else {
        return Err((StatusCode::UNAUTHORIZED, "no pending 2FA".into()));
    };
    let user = load_user(&state, uid).await?;
    let ok = auth::consume_recovery_code(&state.db, &user, &b.code)
        .await
        .map_err(internal)?;
    if !ok {
        ratelimit::record_failure(&key);
        return Err((StatusCode::UNAUTHORIZED, "invalid recovery code".into()));
    }
    ratelimit::record_success(&key);
    let _ = session.remove::<i64>(PENDING_2FA_KEY).await;
    auth_session.login(&user).await.map_err(internal)?;
    let _ = auth::touch_last_login(&state.db, user.id).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn logout(mut auth_session: AuthSession) -> Result<Json<Value>, (StatusCode, String)> {
    auth_session.logout().await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn me(
    State(state): State<Arc<AppState>>,
    session: Session,
    auth_session: AuthSession,
) -> Result<Json<Value>, (StatusCode, String)> {
    let setup_required = auth::count_users(&state.db).await.unwrap_or(0) == 0;
    if setup_required {
        return Ok(Json(json!({
            "setup_required": true,
            "authenticated": false,
        })));
    }
    match auth_session.user {
        Some(user) => Ok(Json(json!({
            "authenticated": true,
            "awaiting_2fa": false,
            "totp_enrolled": user.totp_enrolled == 1,
            "username": user.username,
            "setup_required": false,
        }))),
        None => {
            let pending: Option<i64> = session.get(PENDING_2FA_KEY).await.unwrap_or(None);
            Ok(Json(json!({
                "authenticated": false,
                "awaiting_2fa": pending.is_some(),
                "setup_required": false,
            })))
        }
    }
}

// Log the real error server-side; never echo internals to an (unauthenticated) caller.
fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    tracing::error!("auth route internal error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error".into())
}
fn internal_str(e: impl std::fmt::Display) -> (StatusCode, String) {
    internal(e)
}
