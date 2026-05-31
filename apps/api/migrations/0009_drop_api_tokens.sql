-- Legacy hashed API tokens removed. /api and /mcp now authenticate via the web session
-- (axum-login) or an OIDC access token from the configured external provider.
DROP TABLE IF EXISTS api_tokens;
