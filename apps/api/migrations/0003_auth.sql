-- 0003: built-in auth.
-- Argon2id passwords + TOTP RFC6238 + recovery codes + session cookies.
-- First-run: if `users` empty, the UI shows a setup wizard to register the admin.

CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    username TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,                -- Argon2id PHC string
    totp_secret_enc BLOB,                        -- nullable until 2FA enrolled; ChaCha20-Poly1305
    totp_secret_nonce BLOB,
    totp_enrolled INTEGER NOT NULL DEFAULT 0,
    recovery_codes_enc BLOB,                     -- JSON array of one-use codes; ChaCha20-Poly1305
    recovery_codes_nonce BLOB,
    is_admin INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    last_login_at INTEGER
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,                         -- 32-char URL-safe random
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    ip TEXT,
    user_agent TEXT,
    -- For login flow: a session can be "pending 2FA" until verify-2fa is called.
    awaiting_2fa INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_sessions_user ON sessions(user_id);
CREATE INDEX idx_sessions_expiry ON sessions(expires_at);

-- API tokens for MCP / HA REST sensors / scripted access.
-- These bypass the cookie session and are scoped to a user.
CREATE TABLE api_tokens (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,                          -- "Claude desktop", "HA sensor", etc.
    token_hash TEXT NOT NULL UNIQUE,             -- SHA-256 of the raw token (we never store the raw)
    scopes TEXT NOT NULL DEFAULT 'read',         -- 'read' | 'write' | 'admin'
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
);
CREATE INDEX idx_api_tokens_user ON api_tokens(user_id);
CREATE INDEX idx_api_tokens_revoked ON api_tokens(revoked_at);
