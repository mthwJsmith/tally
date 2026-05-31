-- Remove tables left unused by the auth refactor:
--   * sessions          → replaced by tower-sessions store (axum-login)
--   * oauth_*           → tally is now an OIDC resource server, not an authorization server
-- TOTP single-use replay guard: track the last consumed time-step per user.

DROP TABLE IF EXISTS oauth_access_tokens;
DROP TABLE IF EXISTS oauth_auth_codes;
DROP TABLE IF EXISTS oauth_clients;
DROP TABLE IF EXISTS sessions;

ALTER TABLE users ADD COLUMN last_totp_step INTEGER NOT NULL DEFAULT 0;
