//! OAuth 2.1 authorization server for MCP clients (Claude app, ChatGPT, …).
//!
//! Tally is a *combined* authorization + resource server (common for self-hosted single-user
//! setups). Flow:
//!   1. Client fetches /.well-known/oauth-protected-resource + /.well-known/oauth-authorization-server
//!   2. Client self-registers via POST /oauth/register (RFC 7591 dynamic client registration)
//!   3. Client opens GET /oauth/authorize (PKCE S256) → we show a login+consent page
//!   4. User logs in (Tally username/password + TOTP) and approves → we issue an auth code
//!   5. Client exchanges the code at POST /oauth/token (PKCE verified) for access + refresh tokens
//!   6. /mcp validates the access token.

use crate::auth;
use crate::ratelimit;
use crate::AppState;
use axum::extract::{Form, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const ACCESS_TTL_SECS: i64 = 3600; // 1 hour
const CODE_TTL_SECS: i64 = 600; // 10 minutes

/// Public origin, derived from the request Host (so discovery URLs match what the client used).
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

fn rand_token(n: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(n)
        .map(char::from)
        .collect()
}

// ---------------- discovery ----------------

pub async fn protected_resource_metadata(headers: HeaderMap) -> Json<Value> {
    let base = base_url(&headers);
    Json(json!({
        "resource": format!("{base}/mcp"),
        "authorization_servers": [base],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["read"]
    }))
}

pub async fn authorization_server_metadata(headers: HeaderMap) -> Json<Value> {
    let base = base_url(&headers);
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/oauth/authorize"),
        "token_endpoint": format!("{base}/oauth/token"),
        "registration_endpoint": format!("{base}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": ["read"]
    }))
}

// ---------------- dynamic client registration ----------------

#[derive(Deserialize)]
pub struct RegisterBody {
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
}

pub async fn register(
    State(state): State<Arc<AppState>>,
    Json(b): Json<RegisterBody>,
) -> Response {
    if b.redirect_uris.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_redirect_uri", "error_description": "redirect_uris required"})),
        )
            .into_response();
    }
    let client_id = format!("mcp_{}", rand_token(28));
    let uris_json = serde_json::to_string(&b.redirect_uris).unwrap_or_else(|_| "[]".into());
    if let Err(e) = state
        .db
        .register_oauth_client(&client_id, b.client_name.as_deref(), &uris_json)
        .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}")).into_response();
    }
    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": client_id,
            "redirect_uris": b.redirect_uris,
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "client_name": b.client_name,
        })),
    )
        .into_response()
}

// ---------------- authorize (login + consent) ----------------

#[derive(Deserialize)]
pub struct AuthorizeQuery {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    #[serde(default)]
    pub code_challenge_method: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
}

async fn validate_client(
    state: &AppState,
    client_id: &str,
    redirect_uri: &str,
) -> Result<(), String> {
    let uris = state
        .db
        .get_oauth_client_redirects(client_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "unknown client_id".to_string())?;
    let allowed: Vec<String> = serde_json::from_str(&uris).unwrap_or_default();
    if !allowed.iter().any(|u| u == redirect_uri) {
        return Err("redirect_uri not registered for this client".to_string());
    }
    Ok(())
}

pub async fn authorize_get(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuthorizeQuery>,
) -> Response {
    if q.response_type != "code" {
        return err_page("Unsupported response_type (only \"code\").");
    }
    if q.code_challenge_method.as_deref().unwrap_or("plain") != "S256" {
        return err_page("PKCE S256 is required (code_challenge_method=S256).");
    }
    if let Err(e) = validate_client(&state, &q.client_id, &q.redirect_uri).await {
        return err_page(&e);
    }
    Html(consent_page(&q, None)).into_response()
}

#[derive(Deserialize)]
pub struct AuthorizeForm {
    pub response_type: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge: String,
    pub code_challenge_method: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub resource: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub totp: String,
    pub decision: String, // "approve" | "deny"
}

pub async fn authorize_post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(f): Form<AuthorizeForm>,
) -> Response {
    let rl_key = format!("oauth-authorize:{}", client_ip(&headers));
    if let Err(wait) = ratelimit::check(&rl_key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            format!("too many attempts — retry in {}s", wait.as_secs().max(1)),
        )
            .into_response();
    }
    if validate_client(&state, &f.client_id, &f.redirect_uri)
        .await
        .is_err()
    {
        return err_page("Invalid client or redirect_uri.");
    }
    // Rebuild a query struct for re-rendering the form on error.
    let q = AuthorizeQuery {
        response_type: f.response_type.clone(),
        client_id: f.client_id.clone(),
        redirect_uri: f.redirect_uri.clone(),
        code_challenge: f.code_challenge.clone(),
        code_challenge_method: Some(f.code_challenge_method.clone()),
        state: if f.state.is_empty() { None } else { Some(f.state.clone()) },
        scope: if f.scope.is_empty() { None } else { Some(f.scope.clone()) },
        resource: if f.resource.is_empty() { None } else { Some(f.resource.clone()) },
    };

    if f.decision == "deny" {
        return Redirect::to(&append_params(
            &f.redirect_uri,
            &[("error", "access_denied"), ("state", &f.state)],
        ))
        .into_response();
    }

    // Authenticate the Tally user (password + TOTP if enrolled).
    let user = match auth::find_user_by_username(&state.db, f.username.trim()).await {
        Ok(Some(u)) => u,
        _ => {
            ratelimit::record_failure(&rl_key);
            return Html(consent_page(&q, Some("Wrong username or password."))).into_response();
        }
    };
    if !auth::verify_password(&f.password, &user.password_hash) {
        ratelimit::record_failure(&rl_key);
        return Html(consent_page(&q, Some("Wrong username or password."))).into_response();
    }
    if user.totp_enrolled == 1 {
        let ok = (|| {
            let secret = auth::decrypt_totp_secret(&state.db, &user).ok()?;
            let totp = auth::make_totp(&secret, &user.username).ok()?;
            totp.check_current(f.totp.trim()).ok()
        })()
        .unwrap_or(false);
        if !ok {
            ratelimit::record_failure(&rl_key);
            return Html(consent_page(&q, Some("Invalid 2FA code."))).into_response();
        }
    }
    // Credentials (and 2FA, if enrolled) verified — clear the failure counter.
    ratelimit::record_success(&rl_key);

    // Issue a one-time authorization code bound to the PKCE challenge.
    let code = rand_token(40);
    if let Err(e) = state
        .db
        .create_auth_code(
            &code,
            &f.client_id,
            &f.redirect_uri,
            &f.code_challenge,
            "S256",
            user.id,
            if f.scope.is_empty() { None } else { Some(&f.scope) },
            if f.resource.is_empty() { None } else { Some(&f.resource) },
            CODE_TTL_SECS,
        )
        .await
    {
        return err_page(&format!("Failed to issue code: {e}"));
    }
    Redirect::to(&append_params(
        &f.redirect_uri,
        &[("code", &code), ("state", &f.state)],
    ))
    .into_response()
}

// ---------------- token ----------------

#[derive(Deserialize)]
pub struct TokenForm {
    pub grant_type: String,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub redirect_uri: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub code_verifier: String,
    #[serde(default)]
    pub refresh_token: String,
}

pub async fn token(State(state): State<Arc<AppState>>, Form(f): Form<TokenForm>) -> Response {
    match f.grant_type.as_str() {
        "authorization_code" => {
            let row = match state.db.take_auth_code(&f.code).await {
                Ok(Some(r)) => r,
                Ok(None) => return token_err("invalid_grant", "code invalid or expired"),
                Err(e) => return token_err("server_error", &e.to_string()),
            };
            let (cid, redirect_uri, challenge, user_id, scope) = row;
            if cid != f.client_id {
                return token_err("invalid_grant", "client_id mismatch");
            }
            if redirect_uri != f.redirect_uri {
                return token_err("invalid_grant", "redirect_uri mismatch");
            }
            // PKCE S256: BASE64URL(SHA256(verifier)) == stored challenge.
            let computed = B64URL.encode(Sha256::digest(f.code_verifier.as_bytes()));
            if computed != challenge {
                return token_err("invalid_grant", "PKCE verification failed");
            }
            issue_tokens(&state, &cid, user_id, scope.as_deref()).await
        }
        "refresh_token" => {
            let rhash = auth::hash_api_token(&f.refresh_token);
            let row = match state.db.take_refresh_token(&rhash).await {
                Ok(Some(r)) => r,
                Ok(None) => return token_err("invalid_grant", "refresh token invalid"),
                Err(e) => return token_err("server_error", &e.to_string()),
            };
            let (cid, user_id, scope) = row;
            issue_tokens(&state, &cid, user_id, scope.as_deref()).await
        }
        other => token_err(
            "unsupported_grant_type",
            &format!("grant_type '{other}' not supported"),
        ),
    }
}

async fn issue_tokens(
    state: &AppState,
    client_id: &str,
    user_id: i64,
    scope: Option<&str>,
) -> Response {
    let access = format!("tly_at_{}", rand_token(40));
    let refresh = format!("tly_rt_{}", rand_token(40));
    let access_hash = auth::hash_api_token(&access);
    let refresh_hash = auth::hash_api_token(&refresh);
    if let Err(e) = state
        .db
        .create_oauth_token(&access_hash, &refresh_hash, client_id, user_id, scope, ACCESS_TTL_SECS)
        .await
    {
        return token_err("server_error", &e.to_string());
    }
    Json(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": ACCESS_TTL_SECS,
        "refresh_token": refresh,
        "scope": scope.unwrap_or("read"),
    }))
    .into_response()
}

// ---------------- helpers ----------------

fn token_err(error: &str, desc: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({"error": error, "error_description": desc})),
    )
        .into_response()
}

fn append_params(base: &str, params: &[(&str, &str)]) -> String {
    let pairs: Vec<(&str, &str)> = params
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .copied()
        .collect();
    if pairs.is_empty() {
        return base.to_string();
    }
    let qs = serde_urlencoded::to_string(&pairs).unwrap_or_default();
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}{qs}")
}

fn err_page(msg: &str) -> Response {
    Html(format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Tally — authorization error</title>
        <meta name="viewport" content="width=device-width,initial-scale=1">
        <style>body{{font-family:system-ui,sans-serif;background:#faf6f0;color:#2a1f1a;display:flex;
        min-height:100vh;align-items:center;justify-content:center;margin:0}}
        .card{{background:#fff;border:1px solid #e7ddd1;border-radius:8px;padding:2rem;max-width:28rem}}
        h1{{font-size:1.1rem;margin:0 0 .5rem}} p{{color:#6b5a52}}</style></head>
        <body><div class="card"><h1>Authorization error</h1><p>{}</p></div></body></html>"#,
        html_escape(msg)
    ))
    .into_response()
}

fn consent_page(q: &AuthorizeQuery, error: Option<&str>) -> String {
    let client = html_escape(&q.client_id);
    let err_html = error
        .map(|e| format!(r#"<p style="color:#b3261e;font-size:.85rem">{}</p>"#, html_escape(e)))
        .unwrap_or_default();
    let hidden = |name: &str, val: &str| {
        format!(
            r#"<input type="hidden" name="{}" value="{}">"#,
            name,
            html_escape(val)
        )
    };
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>Authorize — Tally</title>
        <meta name="viewport" content="width=device-width,initial-scale=1">
        <style>
        body{{font-family:system-ui,-apple-system,sans-serif;background:#faf6f0;color:#2a1f1a;
        display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0}}
        .card{{background:#fff;border:1px solid #e7ddd1;border-radius:10px;padding:2rem;width:22rem}}
        h1{{font-size:1.15rem;margin:0 0 .25rem}} .sub{{color:#6b5a52;font-size:.9rem;margin:0 0 1.25rem}}
        label{{display:block;font-size:.7rem;text-transform:uppercase;letter-spacing:.08em;
        color:#6b5a52;margin:.75rem 0 .25rem}}
        input[type=text],input[type=password]{{width:100%;box-sizing:border-box;padding:.55rem .7rem;
        border:1px solid #e7ddd1;border-radius:6px;font-size:.95rem}}
        .row{{display:flex;gap:.5rem;margin-top:1.25rem}}
        button{{flex:1;padding:.6rem;border-radius:6px;border:none;font-weight:600;cursor:pointer;font-size:.9rem}}
        .approve{{background:#505E4D;color:#fff}} .deny{{background:#f0e9df;color:#2a1f1a}}
        b{{color:#2a1f1a}}
        </style></head><body>
        <form class="card" method="post" action="/oauth/authorize">
          <h1>Authorize access</h1>
          <p class="sub"><b>{client}</b> wants to read your Tally data (accounts, transactions,
          bills, net worth). Log in to approve.</p>
          {err_html}
          <label>Username</label><input type="text" name="username" autocomplete="username" autofocus>
          <label>Password</label><input type="password" name="password" autocomplete="current-password">
          <label>2FA code (if enabled)</label><input type="text" name="totp" inputmode="numeric" autocomplete="one-time-code">
          {h_rt}{h_cid}{h_ru}{h_cc}{h_ccm}{h_state}{h_scope}{h_res}
          <div class="row">
            <button class="approve" type="submit" name="decision" value="approve">Approve</button>
            <button class="deny" type="submit" name="decision" value="deny">Deny</button>
          </div>
        </form></body></html>"#,
        client = client,
        err_html = err_html,
        h_rt = hidden("response_type", &q.response_type),
        h_cid = hidden("client_id", &q.client_id),
        h_ru = hidden("redirect_uri", &q.redirect_uri),
        h_cc = hidden("code_challenge", &q.code_challenge),
        h_ccm = hidden("code_challenge_method", q.code_challenge_method.as_deref().unwrap_or("S256")),
        h_state = hidden("state", q.state.as_deref().unwrap_or("")),
        h_scope = hidden("scope", q.scope.as_deref().unwrap_or("")),
        h_res = hidden("resource", q.resource.as_deref().unwrap_or("")),
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
