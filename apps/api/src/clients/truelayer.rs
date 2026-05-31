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
        if !consent.is_access_token_expired() {
            return db.decrypt_access_token(consent);
        }
        let refresh = db.decrypt_refresh_token(consent)?;
        let new_tokens = self.refresh(&refresh).await?;
        let expires_at = Utc::now().timestamp() + new_tokens.expires_in;
        db.update_tokens(
            consent.id,
            &new_tokens.access_token,
            &new_tokens.refresh_token,
            expires_at,
        )
        .await?;
        Ok(new_tokens.access_token)
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
