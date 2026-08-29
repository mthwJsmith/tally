//! OIDC resource-server validation for `/mcp`.
//!
//! Per the 2025-06-18 MCP authorization spec, tally is a *resource server*: it validates
//! access tokens minted by an external authorization server (Authentik / Keycloak / …) and
//! never issues tokens itself. Tokens are JWTs verified against the IdP's JWKS (signature,
//! issuer, audience, expiry). The JWKS is fetched lazily and cached, with a forced refresh on
//! an unknown `kid` (key rotation).

use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
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

/// Single-user self-host escape hatch shared by `/mcp` and the REST middleware: when the owner
/// sets TALLY_MCP_WRITE truthy, any token that PASSES full OIDC validation (i.e. is the owner)
/// may write without the IdP minting a separate `write` scope. Default (unset) keeps the strict
/// scope gate. Validation itself is never bypassed.
pub fn write_allowed_by_env() -> bool {
    std::env::var("TALLY_MCP_WRITE")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
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

    // Pin the algorithm allowlist to the key's own family (asymmetric only) rather than
    // trusting the token's `alg` header — this prevents algorithm-confusion attacks (e.g. an
    // `HS256` token forged with the public key). The list must contain ONLY algorithms of the
    // key's family: jsonwebtoken 9.3+ rejects the whole request with `InvalidAlgorithm` if any
    // listed algorithm's family differs from the key's, so RSA and EC cannot be mixed here.
    let algorithms = match &jwk.algorithm {
        AlgorithmParameters::RSA(_) => {
            vec![Algorithm::RS256, Algorithm::RS384, Algorithm::RS512]
        }
        AlgorithmParameters::EllipticCurve(_) => vec![Algorithm::ES256, Algorithm::ES384],
        // Symmetric / unsupported key types are never acceptable for token validation here.
        _ => return None,
    };
    let mut validation = Validation::new(algorithms[0]);
    validation.algorithms = algorithms;
    validation.set_issuer(&[cfg.issuer.as_str()]);
    validation.set_audience(&[cfg.audience.as_str()]);
    // set_issuer/set_audience only check the claims when present; require them so a token
    // that simply omits aud/iss can't skate past the checks.
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    let data = decode::<Claims>(token, &key, &validation).ok()?;
    Some(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(scope: &str, scp: &[&str]) -> Claims {
        Claims {
            sub: "u".into(),
            scope: scope.into(),
            scp: scp.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn has_scope_reads_space_delimited_and_array() {
        let c = claims("read write", &[]);
        assert!(c.has_scope("read"));
        assert!(c.has_scope("write"));
        assert!(!c.has_scope("admin"));

        let c2 = claims("", &["read"]);
        assert!(c2.has_scope("read"));
        assert!(!c2.has_scope("write")); // read-only token must NOT grant write

        assert!(!claims("", &[]).has_scope("read")); // no scope → nothing
    }

    #[tokio::test]
    async fn validate_rejects_malformed_tokens_without_network() {
        let cfg = OidcConfig {
            issuer: "https://issuer.example".into(),
            audience: "aud".into(),
            // unroutable — proves these rejections happen before any JWKS fetch
            jwks_url: "http://127.0.0.1:1/jwks".into(),
        };
        assert!(validate(&cfg, "not-a-jwt").await.is_none());
        assert!(validate(&cfg, "").await.is_none());
        // alg:"none" header fails to parse / has no kid → rejected, no fetch attempted
        assert!(validate(&cfg, "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJ4In0.")
            .await
            .is_none());
    }
}
