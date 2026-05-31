//! tally — self-hosted personal finance OS.
//! Multi-bank UK Open Banking sync (TrueLayer + Plaid) → local SQLite → TanStack web UI.

mod ai;
mod auth;
mod clients;
mod crypto;
mod db;
mod importer;
mod middleware;
mod models;
mod notifier;
mod ratelimit;
mod routes;
mod rules;
mod scheduler;

use crate::clients::truelayer::TrueLayerClient;
use crate::crypto::Crypto;
use crate::db::Db;
use crate::notifier::Notifier;
use anyhow::{Context, Result};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub tl: TrueLayerClient,
    pub notifier: Notifier,
    pub ai: ai::AiClient,
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
    let tl = TrueLayerClient::new(tl_client_id, tl_client_secret, tl_redirect_base.clone())?;
    let notifier = Notifier::from_env();

    let ai = ai::AiClient::new(db.clone());
    let state = Arc::new(AppState { db, tl, notifier, ai });

    let _sched = scheduler::start_scheduler(state.clone()).await?;

    let app = routes::router(state.clone()).layer(TraceLayer::new_for_http());

    let addr: SocketAddr = format!("0.0.0.0:{port}").parse()?;
    tracing::info!("tally listening on http://{addr}");
    tracing::info!("OAuth redirect base: {tl_redirect_base}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
