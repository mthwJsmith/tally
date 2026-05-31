-- OAuth 2.1 authorization-server tables, so Tally can act as a combined AS + resource server
-- for MCP clients (Claude app, ChatGPT, etc.). Single-user: consent reuses the Tally session.

-- Dynamically-registered clients (RFC 7591). Public clients (PKCE), no secret stored.
CREATE TABLE oauth_clients (
    client_id     TEXT PRIMARY KEY,
    client_name   TEXT,
    redirect_uris TEXT NOT NULL,          -- JSON array of allowed redirect URIs
    created_at    INTEGER NOT NULL
);

-- Short-lived authorization codes (one-time, PKCE-bound).
CREATE TABLE oauth_auth_codes (
    code                  TEXT PRIMARY KEY,   -- random opaque
    client_id             TEXT NOT NULL,
    redirect_uri          TEXT NOT NULL,
    code_challenge        TEXT NOT NULL,
    code_challenge_method TEXT NOT NULL,      -- always S256
    user_id               INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scope                 TEXT,
    resource              TEXT,               -- RFC 8707 resource indicator
    expires_at            INTEGER NOT NULL,
    created_at            INTEGER NOT NULL
);

-- Access + refresh tokens. We store only SHA-256 hashes (same scheme as api_tokens).
CREATE TABLE oauth_access_tokens (
    token_hash         TEXT PRIMARY KEY,
    refresh_token_hash TEXT UNIQUE,
    client_id          TEXT NOT NULL,
    user_id            INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scope              TEXT,
    expires_at         INTEGER NOT NULL,      -- access-token expiry
    created_at         INTEGER NOT NULL,
    revoked_at         INTEGER
);
CREATE INDEX idx_oauth_tokens_refresh ON oauth_access_tokens(refresh_token_hash);
