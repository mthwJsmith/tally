//! tally — self-hosted personal finance OS.
//! Multi-bank UK Open Banking sync (TrueLayer + Plaid) → local SQLite → TanStack web UI.

mod ai;
mod auth;
mod auth_backend;
mod clients;
mod crypto;
mod db;
mod importer;
mod middleware;
mod models;
mod notifier;
mod oidc;
mod ratelimit;
mod recurrence;
mod routes;
mod rules;
mod scheduler;

use crate::clients::truelayer::TrueLayerClient;
use crate::crypto::Crypto;
use crate::db::Db;
use crate::notifier::Notifier;
use anyhow::{Context, Result};
use axum::http::{header, HeaderValue};
use axum_login::AuthManagerLayerBuilder;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use tower_sessions::cookie::SameSite;
use tower_sessions::{Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::SqliteStore;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub tl: TrueLayerClient,
    pub notifier: Notifier,
    pub ai: ai::AiClient,
    /// External OIDC authorization server for `/mcp` (None disables the JWT path).
    pub oidc: Option<oidc::OidcConfig>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,tally=debug")),
        )
        .with_target(false)
        .compact()
        .init();

    let port: u16 = env::var("TALLY_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3001);

    let database_url =
        env::var("TALLY_DATABASE_URL").unwrap_or_else(|_| "sqlite:///app/data/state.db".into());

    let tl_client_id =
        env::var("TALLY_TRUELAYER_CLIENT_ID").context("TALLY_TRUELAYER_CLIENT_ID must be set")?;
    let tl_client_secret = env::var("TALLY_TRUELAYER_CLIENT_SECRET")
        .context("TALLY_TRUELAYER_CLIENT_SECRET must be set")?;
    let tl_redirect_base = env::var("TALLY_REDIRECT_URI_BASE").unwrap_or_else(|_| {
        format!("http://localhost:{port}/auth")
    });

    let crypto = Crypto::from_env()?;
    let db = Db::connect(&database_url, crypto).await?;

    // Session store + axum-login auth layer (replaces the hand-rolled `sessions` table).
    let session_store = SqliteStore::new(db.pool.clone());
    session_store
        .migrate()
        .await
        .context("migrating session store")?;
    let secure_cookies = env::var("TALLY_SECURE_COOKIES")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(secure_cookies)
        .with_same_site(SameSite::Strict)
        .with_expiry(Expiry::OnInactivity(time::Duration::days(30)));
    let auth_layer =
        AuthManagerLayerBuilder::new(auth_backend::Backend::new(db.clone()), session_layer).build();

    let tl = TrueLayerClient::new(tl_client_id, tl_client_secret, tl_redirect_base.clone())?;
    let mut notifier = Notifier::from_env();
    notifier.db = Some(db.clone()); // let Settings-saved Telegram creds override env

    let ai = ai::AiClient::new(db.clone());
    let oidc = oidc::OidcConfig::from_env();
    match &oidc {
        Some(c) => tracing::info!("MCP OIDC resource server: issuer={}", c.issuer),
        None => tracing::info!("MCP OIDC disabled (set TALLY_OIDC_ISSUER/AUDIENCE to enable); /mcp rejects all requests"),
    }
    let state = Arc::new(AppState {
        db,
        tl,
        notifier,
        ai,
        oidc,
    });

    let _sched = scheduler::start_scheduler(state.clone()).await?;

    let app = routes::router(state.clone())
        .layer(auth_layer)
        // Security headers.
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; \
                 img-src 'self' data:; \
                 style-src 'self' 'unsafe-inline'; \
                 script-src 'self'; \
                 connect-src 'self'; \
                 font-src 'self' data:; \
                 base-uri 'self'; \
                 form-action 'self'; \
                 frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ))
        .layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    tracing::info!("tally listening on http://{addr}");
    tracing::info!("OAuth redirect base: {tl_redirect_base}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    // with_connect_info: handlers need the TCP peer address for rate-limit keys.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
