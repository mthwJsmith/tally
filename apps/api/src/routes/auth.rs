//! OAuth callback handler. Receives the `code` + `state` from TrueLayer, validates the state
//! (which maps back to a consent nickname we saved before redirect), exchanges code for tokens,
//! persists the consent.
//!
//! One redirect URI for all banks — the nickname is carried in the state token, not the URL path.

use crate::clients::truelayer::TrueLayerClient;
use crate::AppState;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Redirect};
use chrono::{Duration, Utc};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct CallbackParams {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

pub async fn callback(
    State(state): State<Arc<AppState>>,
    Query(params): Query<CallbackParams>,
) -> impl IntoResponse {
    if let Some(err) = params.error {
        tracing::warn!("OAuth callback error: {err} - {:?}", params.error_description);
        // The value is attacker-controlled; strip it to a safe charset (and cap the length)
        // so it can't smuggle header-invalid bytes into Redirect::to, which panics on them.
        let err: String = err
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .take(64)
            .collect();
        return Redirect::to(&format!("/?error={err}"));
    }
    let Some(code) = params.code else {
        return Redirect::to("/?error=missing_code");
    };
    let Some(state_token) = params.state else {
        return Redirect::to("/?error=missing_state");
    };

    // Validate state -> nickname.
    let nickname = match state.db.consume_oauth_state(&state_token).await {
        Ok(Some(n)) => n,
        Ok(None) => {
            tracing::warn!("OAuth state not found or already consumed: {state_token}");
            return Redirect::to("/?error=invalid_state");
        }
        Err(e) => {
            tracing::error!("consume_oauth_state failed: {e:#}");
            return Redirect::to("/?error=db");
        }
    };

    // Exchange code for tokens.
    let token_resp = match state.tl.exchange_code(&code).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("exchange_code failed: {e:#}");
            return Redirect::to("/?error=exchange_failed");
        }
    };

    let (credentials_id, _exp_unused) =
        match TrueLayerClient::decode_token_claims(&token_resp.access_token) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("decode JWT claims failed (continuing): {e:#}");
                (String::from("unknown"), 0)
            }
        };

    let now = Utc::now().timestamp();
    let expires_at = now + token_resp.expires_in;
    // Consent expires per PSD2 in 90 days — best-effort; refresh-token loses validity then.
    let consent_expires_at = (Utc::now() + Duration::days(90)).timestamp();

    let scopes = token_resp
        .scope
        .clone()
        .unwrap_or_else(|| "info accounts balance cards transactions direct_debits standing_orders offline_access".into());

    match state
        .db
        .upsert_consent(
            &nickname,
            &credentials_id,
            None,
            None,
            &token_resp.access_token,
            &token_resp.refresh_token,
            expires_at,
            Some(consent_expires_at),
            &scopes,
        )
        .await
    {
        Ok(consent) => {
            tracing::info!(
                consent.nickname = %consent.nickname,
                consent.id = consent.id,
                "consent saved"
            );
            // Trigger an immediate first sync in background.
            let app_state = state.clone();
            let cid = consent.id;
            tokio::spawn(async move {
                if let Ok(Some(c)) = app_state.db.get_consent(cid).await {
                    let importer = crate::importer::Importer {
                        db: app_state.db.clone(),
                        tl: app_state.tl.clone(),
                        notifier: app_state.notifier.clone(),
                        ai: app_state.ai.clone(),
                    };
                    if let Err(e) = importer.sync_consent(&c).await {
                        tracing::error!("first sync after consent failed: {e:#}");
                    }
                }
            });
            // Land on /banks where they can see the sync progress — dashboard would otherwise
            // show empty until the background sync settles.
            Redirect::to(&format!("/banks?linked={}", consent.nickname))
        }
        Err(e) => {
            tracing::error!("upsert_consent failed: {e:#}");
            Redirect::to("/?error=save_failed")
        }
    }
}
