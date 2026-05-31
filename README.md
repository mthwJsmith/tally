# tally

Self-hosted personal finance for UK Open Banking. Rust (axum) backend, React frontend, one
Docker container, SQLite storage with secrets encrypted at rest. It includes an MCP server so AI
assistants can read your finances, and built-in 2FA.

## Before you expose it publicly

tally aggregates your bank data, so treat it like a sensitive internal app. If it will be
reachable from the internet, put a zero-trust layer in front of it. The easiest option is
Cloudflare Zero Trust (Cloudflare Access) on your domain; an identity provider with forward-auth
(Authentik, Authelia, Keycloak) works the same way. Gate the web UI, and leave only the paths
that machines need reachable: `/mcp`, `/.well-known/*`, and `/auth/callback` (the bank redirect).

You do not have to rely on that layer alone. tally also protects itself: Argon2id password
hashing, TOTP 2FA with single-use codes, per-IP login rate limiting with lockout, ChaCha20-Poly1305
encryption of tokens at rest, and standard security headers. Run it behind a TLS reverse proxy and
set `TALLY_SECURE_COOKIES=true`.

## What it does

- Syncs UK bank accounts through TrueLayer. Multi-bank with a single OAuth callback URL.
- Stores every transaction locally in SQLite, not just references.
- Rules engine (regex to category), budgets, bills, and search/aggregate APIs.
- Tracks investment holdings and net worth.
- Exposes an MCP server so Claude, ChatGPT, and similar clients can query your data.
- Has its own auth: first-run setup, Argon2id passwords, and TOTP 2FA with recovery codes.

## Requirements

- Docker and Docker Compose.
- A TrueLayer Live application (client id and secret) from console.truelayer.com.
- For the MCP connector, an OIDC provider that issues access tokens (Authentik, Keycloak, Auth0,
  and so on). tally validates those tokens; it does not run its own OAuth server.

## Quickstart

```bash
git clone https://github.com/mthwjsmith/tally.git
cd tally
cp apps/api/.env.example .env
# set TALLY_MASTER_KEY  (openssl rand -base64 32)
# set TALLY_TRUELAYER_CLIENT_ID and TALLY_TRUELAYER_CLIENT_SECRET
docker compose up -d --build
# open http://localhost:3001, create the first user, enrol 2FA, then add a bank
```

The container binds to `127.0.0.1:3001` by default. For remote access, front it with a TLS reverse
proxy (and the zero-trust layer above).

## Configuration

Full list is in `apps/api/.env.example`. The ones you usually need:

| Variable | Purpose |
|----------|---------|
| `TALLY_MASTER_KEY` | 32-byte base64 key that encrypts tokens at rest (required) |
| `TALLY_TRUELAYER_CLIENT_ID` / `_SECRET` | TrueLayer Live credentials (required) |
| `TALLY_REDIRECT_URI_BASE` | Public base URL TrueLayer redirects back to |
| `TALLY_OIDC_ISSUER` / `TALLY_OIDC_AUDIENCE` | External OIDC provider for the MCP connector |
| `TALLY_SECURE_COOKIES` | Set to `true` when served over HTTPS |

## MCP connector

`/mcp` accepts access tokens issued by your OIDC provider and validates them against the
provider's JWKS (signature, issuer, audience, expiry). To set it up, create an OAuth2/OIDC
application in your provider, set `TALLY_OIDC_ISSUER` and `TALLY_OIDC_AUDIENCE` to match, then add
the connector in your AI client. The read tools accept any valid token. The single write tool
(`add_investment_activity`) requires a token with the `write` scope.

A ready-to-run Authentik stack is included:

```bash
docker compose -f docker-compose.yml -f docker-compose.idp.yml up -d
```

## Build from source

- Backend: Rust stable, then `cargo build --release` in `apps/api`.
- Frontend: Node 22, then `npm install` and `npm run build` in `apps/web`.

The Dockerfile builds both stages and bundles the built SPA into the binary's static directory.

## Security reporting

See `SECURITY.md`. Use private vulnerability reporting rather than public issues.

## License

MIT. See `LICENSE`.
