//! OIDC resource-server validation for `/mcp`.
//!
//! Per the 2025-06-18 MCP authorization spec, tally is a *resource server*: it validates
//! access tokens minted by an external authorization server (Authentik / Keycloak / …) and
//! never issues tokens itself. Tokens are JWTs verified against the IdP's JWKS (signature,
//! issuer, audience, expiry). The JWKS is fetched lazily and cached, with a forced refresh on
//! an unknown `kid` (key rotation).

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// External authorization-server config. `None` (unset env) disables the JWT path, leaving
/// only the legacy API-token auth on `/mcp`.
#[derive(Clone, Debug)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    pub jwks_url: String,
}

impl OidcConfig {
    pub fn from_env() -> Option<Self> {
        let issuer = std::env::var("TALLY_OIDC_ISSUER").ok()?;
        let issuer = issuer.trim_end_matches('/').to_string();
        let audience = std::env::var("TALLY_OIDC_AUDIENCE").ok()?;
        let jwks_url = std::env::var("TALLY_OIDC_JWKS_URL")
            .ok()
            .unwrap_or_else(|| format!("{issuer}/jwks/"));
        Some(Self {
            issuer,
            audience,
            jwks_url,
        })
    }
}

/// Validated token claims we care about. `scope` (space-delimited) or `scp` (array) per IdP.
#[derive(Debug, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub scp: Vec<String>,
}

impl Claims {
    /// True if the token carries `scope`/`scp` granting the named scope.
    pub fn has_scope(&self, want: &str) -> bool {
        self.scope.split_whitespace().any(|s| s == want) || self.scp.iter().any(|s| s == want)
    }
}

struct CachedJwks {
    set: JwkSet,
    fetched: Instant,
}

static JWKS_CACHE: OnceLock<Mutex<Option<CachedJwks>>> = OnceLock::new();
const JWKS_TTL: Duration = Duration::from_secs(3600);

async fn fetch_jwks(url: &str) -> Option<JwkSet> {
    let client = reqwest::Client::new();
    let resp = client.get(url).send().await.ok()?;
    resp.json::<JwkSet>().await.ok()
}

/// Return the cached JWKS, refreshing if stale or `force`d.
async fn jwks(cfg: &OidcConfig, force: bool) -> Option<JwkSet> {
    let cache = JWKS_CACHE.get_or_init(|| Mutex::new(None));
    if !force {
        if let Some(c) = cache.lock().ok()?.as_ref() {
            if c.fetched.elapsed() < JWKS_TTL {
                return Some(c.set.clone());
            }
        }
    }
    let set = fetch_jwks(&cfg.jwks_url).await?;
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(CachedJwks {
            set: set.clone(),
            fetched: Instant::now(),
        });
    }
    Some(set)
}

/// Validate a Bearer JWT against the IdP. Returns the claims on success, `None` otherwise.
pub async fn validate(cfg: &OidcConfig, token: &str) -> Option<Claims> {
    let header = decode_header(token).ok()?;
    let kid = header.kid?;

    // Try the cached key; on a miss, force a JWKS refresh once (handles key rotation).
    let mut set = jwks(cfg, false).await?;
    if set.find(&kid).is_none() {
        set = jwks(cfg, true).await?;
    }
    let jwk = set.find(&kid)?;
    let key = DecodingKey::from_jwk(jwk).ok()?;

    // Pin an asymmetric-only algorithm allowlist rather than trusting the token's own `alg`
    // header — this prevents algorithm-confusion attacks (e.g. an `HS256` token forged with the
    // public key). IdPs like Authentik/Keycloak sign with RS256 by default.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.algorithms = vec![
        Algorithm::RS256,
        Algorithm::RS384,
        Algorithm::RS512,
        Algorithm::ES256,
        Algorithm::ES384,
    ];
    validation.set_issuer(&[cfg.issuer.as_str()]);
    validation.set_audience(&[cfg.audience.as_str()]);
    let data = decode::<Claims>(token, &key, &validation).ok()?;
    Some(data.claims)
}
