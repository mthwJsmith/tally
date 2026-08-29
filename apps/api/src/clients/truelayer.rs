//! TrueLayer Data API client.
//!
//! Per-consent contextual — each method takes either the consent (for token refresh logic)
//! or an explicit access_token (during OAuth bootstrap before consent is persisted).

use crate::db::Db;
use crate::models::truelayer::*;
use crate::models::Consent;
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use jsonwebtoken::{decode, DecodingKey, Validation};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::Mutex;

const AUTH_BASE: &str = "https://auth.truelayer.com";
const DATA_BASE: &str = "https://api.truelayer.com/data/v1";
const USER_AGENT: &str = concat!("tally/", env!("CARGO_PKG_VERSION"));

/// The full OAuth scope set we request. Matches what's documented in TRUELAYER-API.md.
pub const SCOPE: &str = "info accounts balance cards transactions direct_debits standing_orders offline_access";

/// UK providers — covers all CMA9 plus NewDay (Aqua) etc.
pub const UK_PROVIDERS: &str = "uk-ob-all uk-oauth-all";

#[derive(Clone)]
pub struct TrueLayerClient {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri_base: String, // e.g. https://tally.example.com/auth
    http: Client,
    /// One lock per consent id, guarding token refresh.
    ///
    /// A single sync makes a dozen-odd Data API calls, and each one asks for a fresh access
    /// token while holding the *same* `Consent` snapshot it was handed at the top of the sync.
    /// Without this, every one of those calls sees the stale `expires_at`, decides the token
    /// is expired, and posts the same refresh token to `/connect/token` again — a dozen
    /// identical token requests per bank per hour. Serialising on the consent and re-reading
    /// the row inside the lock collapses that to exactly one.
    refresh_locks: Arc<StdMutex<HashMap<i64, Arc<Mutex<()>>>>>,
}

/// TrueLayer answered `invalid_grant` on `/connect/token`: the consent itself is dead, not the
/// access token. Under PSD2 a consent lasts 90 days, after which the bank requires the user to
/// re-authenticate in person (SCA); the user revoking access at the bank looks identical.
/// Neither is recoverable by retrying — only a fresh OAuth flow brings the link back.
#[derive(Debug, thiserror::Error)]
#[error("TrueLayer consent is no longer valid (invalid_grant) — this bank must be re-linked")]
pub struct ConsentRevoked;

/// True if `err` (or anything it wraps) is a dead consent.
pub fn is_consent_revoked(err: &anyhow::Error) -> bool {
    err.chain().any(|e| e.is::<ConsentRevoked>())
}

#[derive(Debug, Deserialize)]
struct AccessTokenClaims {
    sub: String,
    exp: i64,
}

impl TrueLayerClient {
    pub fn new(client_id: String, client_secret: String, redirect_uri_base: String) -> Result<Self> {
        let http = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client_id,
            client_secret,
            redirect_uri_base,
            http,
            refresh_locks: Arc::new(StdMutex::new(HashMap::new())),
        })
    }

    /// Single OAuth redirect URI — the consent nickname is carried in the `state` token,
    /// not the URL path. This means you only register ONE redirect URI in the TrueLayer console.
    pub fn redirect_uri(&self) -> String {
        format!("{}/callback", self.redirect_uri_base.trim_end_matches('/'))
    }

    /// Build the URL the user's browser is redirected to for OAuth.
    pub fn build_authorize_url(&self, state: &str) -> String {
        let nonce = Utc::now().timestamp().to_string();
        let redirect = self.redirect_uri();
        let params = [
            ("response_type", "code"),
            ("client_id", &self.client_id),
            ("redirect_uri", &redirect),
            ("nonce", &nonce),
            ("scope", SCOPE),
            ("providers", UK_PROVIDERS),
            ("state", state),
        ];
        let qs = serde_urlencoded::to_string(params).unwrap_or_default();
        format!("{AUTH_BASE}/?{qs}")
    }

    /// Exchange the OAuth `code` for an access+refresh token pair.
    pub async fn exchange_code(&self, code: &str) -> Result<TokenResponse> {
        let redirect = self.redirect_uri();
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
            ("redirect_uri", &redirect),
            ("code", code),
        ];
        let resp = self
            .http
            .post(format!("{AUTH_BASE}/connect/token"))
            .form(&form)
            .send()
            .await
            .context("posting to /connect/token")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "exchange_code failed: status {status}, body: {body}"
            ));
        }
        let token: TokenResponse = resp.json().await.context("parsing token response")?;
        Ok(token)
    }

    /// Refresh tokens.
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenResponse> {
        let form = [
            ("grant_type", "refresh_token"),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
            ("refresh_token", refresh_token),
        ];
        let resp = self
            .http
            .post(format!("{AUTH_BASE}/connect/token"))
            .form(&form)
            .send()
            .await
            .context("posting refresh request")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            if body.contains("invalid_grant") {
                return Err(anyhow::Error::new(ConsentRevoked)
                    .context(format!("refresh failed: status {status}, body: {body}")));
            }
            return Err(anyhow!("refresh failed: status {status}, body: {body}"));
        }
        Ok(resp.json().await.context("parsing refresh response")?)
    }

    /// Decode the access-token JWT (no signature verification) to extract `sub` (credentials_id)
    /// and `exp` (expiry timestamp).
    pub fn decode_token_claims(token: &str) -> Result<(String, i64)> {
        let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
        validation.insecure_disable_signature_validation();
        validation.required_spec_claims.clear();
        validation.validate_exp = false;
        let key = DecodingKey::from_secret(&[]);
        let data = decode::<AccessTokenClaims>(token, &key, &validation)
            .context("decoding TL access token JWT")?;
        Ok((data.claims.sub, data.claims.exp))
    }

    // ---------- authenticated calls ----------

    /// Returns a valid access token for the consent, refreshing if needed.
    /// Mutates the DB if a refresh occurs.
    pub async fn fresh_access_token(&self, db: &Db, consent: &Consent) -> Result<String> {
        // Fast path: the snapshot we were handed is still good.
        if !consent.is_access_token_expired() {
            return db.decrypt_access_token(consent);
        }

        let lock = self.refresh_lock(consent.id);
        let _guard = lock.lock().await;

        // Re-read under the lock. An earlier call in this same sync may already have
        // refreshed, in which case the caller's snapshot — and the refresh token inside it —
        // is stale, and refreshing off it would be a pointless second round trip.
        let current = db
            .get_consent(consent.id)
            .await?
            .ok_or_else(|| anyhow!("consent {} no longer exists", consent.id))?;
        if !current.is_access_token_expired() {
            return db.decrypt_access_token(&current);
        }

        let refresh = db.decrypt_refresh_token(&current)?;
        let new_tokens = self.refresh(&refresh).await?;
        let expires_at = Utc::now().timestamp() + new_tokens.expires_in;
        db.update_tokens(
            current.id,
            &new_tokens.access_token,
            &new_tokens.refresh_token,
            expires_at,
        )
        .await?;
        Ok(new_tokens.access_token)
    }

    fn refresh_lock(&self, consent_id: i64) -> Arc<Mutex<()>> {
        let mut locks = self
            .refresh_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.entry(consent_id).or_default().clone()
    }

    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        access_token: &str,
        path: &str,
    ) -> Result<T> {
        let url = format!("{DATA_BASE}/{}", path.trim_start_matches('/'));
        let resp = self
            .http
            .get(&url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("{url} failed: status {status}, body: {body}"));
        }
        Ok(resp.json::<T>().await.with_context(|| format!("parsing JSON from {url}"))?)
    }

    // ---------- account endpoints ----------

    pub async fn get_accounts(&self, db: &Db, c: &Consent) -> Result<Vec<TLAccount>> {
        let token = self.fresh_access_token(db, c).await?;
        // Credit-card-only providers (e.g. Capital One) have no current accounts and return
        // 501 "endpoint_not_supported" here. That must NOT fail the whole sync — their data
        // lives under /cards, which is fetched next. Mirror get_cards' tolerance.
        let url = format!("{DATA_BASE}/accounts");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if resp.status().as_u16() == 501 || resp.status().as_u16() == 403 {
            tracing::debug!("accounts endpoint not supported by provider; skipping (card-only?)");
            return Ok(vec![]);
        }
        if !resp.status().is_success() {
            return Err(anyhow!(
                "get_accounts failed: {} body: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }
        let body: ApiResponse<TLAccount> = resp.json().await?;
        Ok(body.results)
    }

    pub async fn get_cards(&self, db: &Db, c: &Consent) -> Result<Vec<TLCard>> {
        let token = self.fresh_access_token(db, c).await?;
        // some providers return 501 here if cards scope unavailable; treat gracefully.
        let url = format!("{DATA_BASE}/cards");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if resp.status().as_u16() == 501 || resp.status().as_u16() == 403 {
            tracing::debug!("cards endpoint not supported by provider; skipping");
            return Ok(vec![]);
        }
        if !resp.status().is_success() {
            return Err(anyhow!(
                "get_cards failed: {} body: {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            ));
        }
        let body: ApiResponse<TLCard> = resp.json().await?;
        Ok(body.results)
    }

    pub async fn get_account_transactions(
        &self,
        db: &Db,
        c: &Consent,
        account_id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<TLTransaction>> {
        let token = self.fresh_access_token(db, c).await?;
        let mut path = format!("accounts/{account_id}/transactions");
        if let (Some(f), Some(t)) = (from, to) {
            path = format!("{path}?from={f}&to={t}");
        }
        let resp: ApiResponse<TLTransaction> = self.get_json(&token, &path).await?;
        Ok(resp.results)
    }

    pub async fn get_account_balance(
        &self,
        db: &Db,
        c: &Consent,
        account_id: &str,
    ) -> Result<TLAccountBalance> {
        let token = self.fresh_access_token(db, c).await?;
        let path = format!("accounts/{account_id}/balance");
        let resp: ApiResponse<TLAccountBalance> = self.get_json(&token, &path).await?;
        resp.results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("empty balance response for account {account_id}"))
    }

    pub async fn get_card_balance(
        &self,
        db: &Db,
        c: &Consent,
        card_id: &str,
    ) -> Result<TLCardBalance> {
        let token = self.fresh_access_token(db, c).await?;
        let path = format!("cards/{card_id}/balance");
        let resp: ApiResponse<TLCardBalance> = self.get_json(&token, &path).await?;
        resp.results
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("empty balance response for card {card_id}"))
    }

    pub async fn get_account_pending(
        &self,
        db: &Db,
        c: &Consent,
        account_id: &str,
    ) -> Result<Vec<TLTransaction>> {
        let token = self.fresh_access_token(db, c).await?;
        // Some banks return 501 if pending unsupported — fail gracefully.
        let url = format!("{DATA_BASE}/accounts/{account_id}/transactions/pending");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            tracing::debug!(
                "pending transactions unavailable for {account_id} ({})",
                resp.status()
            );
            return Ok(vec![]);
        }
        let body: ApiResponse<TLTransaction> = resp.json().await?;
        Ok(body.results)
    }

    pub async fn get_card_transactions(
        &self,
        db: &Db,
        c: &Consent,
        card_id: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<Vec<TLTransaction>> {
        let token = self.fresh_access_token(db, c).await?;
        let mut path = format!("cards/{card_id}/transactions");
        if let (Some(f), Some(t)) = (from, to) {
            path = format!("{path}?from={f}&to={t}");
        }
        let resp: ApiResponse<TLTransaction> = self.get_json(&token, &path).await?;
        Ok(resp.results)
    }

    pub async fn get_card_pending(
        &self,
        db: &Db,
        c: &Consent,
        card_id: &str,
    ) -> Result<Vec<TLTransaction>> {
        let token = self.fresh_access_token(db, c).await?;
        let url = format!("{DATA_BASE}/cards/{card_id}/transactions/pending");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let body: ApiResponse<TLTransaction> = resp.json().await?;
        Ok(body.results)
    }

    pub async fn get_standing_orders(
        &self,
        db: &Db,
        c: &Consent,
        account_id: &str,
    ) -> Result<Vec<TLStandingOrder>> {
        let token = self.fresh_access_token(db, c).await?;
        let url = format!("{DATA_BASE}/accounts/{account_id}/standing_orders");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let body: ApiResponse<TLStandingOrder> = resp.json().await?;
        Ok(body.results)
    }

    pub async fn get_direct_debits(
        &self,
        db: &Db,
        c: &Consent,
        account_id: &str,
    ) -> Result<Vec<TLDirectDebit>> {
        let token = self.fresh_access_token(db, c).await?;
        let url = format!("{DATA_BASE}/accounts/{account_id}/direct_debits");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .header("Accept", "application/json")
            .send()
            .await?;
        if !resp.status().is_success() {
            return Ok(vec![]);
        }
        let body: ApiResponse<TLDirectDebit> = resp.json().await?;
        Ok(body.results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoked_consent_survives_context_wrapping() {
        // The importer sees this error only after `.context("get_accounts")` and the
        // `{e:#}` formatting in between, so the marker has to be found by downcast down
        // the whole chain rather than by matching on the rendered message.
        let err = anyhow::Error::new(ConsentRevoked)
            .context("refresh failed: status 400 Bad Request")
            .context("get_accounts");
        assert!(is_consent_revoked(&err));
    }

    #[test]
    fn ordinary_failures_are_not_treated_as_revoked() {
        // A 500 or a timeout must stay retryable — flagging it `reauth` would park a
        // healthy bank until the user needlessly re-linked it.
        let err = anyhow!("refresh failed: status 503 Service Unavailable")
            .context("get_accounts");
        assert!(!is_consent_revoked(&err));
    }
}
