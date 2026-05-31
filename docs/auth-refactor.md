# Auth hardening & refactor — design doc

Status: **in progress** · Goal: replace hand-rolled auth plumbing with maintained libraries and the spec-correct MCP authorization model, so the only bespoke code left is thin glue.

## Why

A pre-release security audit found two critical wiring bugs in the bespoke auth/OAuth code (an unauthenticated admin route group and a TOTP re-enrolment bypass — both now patched) plus a class of "we rolled our own" risk. The crypto primitives are fine (`argon2`, `totp-rs`, `chacha20poly1305`); the *plumbing* (session table, login flow, route guards, and a full custom OAuth 2.1 authorization server) is where bugs hide. This refactor moves that plumbing onto vetted crates and the current MCP spec.

## Target architecture

```
                 ┌─────────────────────────────────────────────┐
   Browser  ───▶ │  Reverse proxy (Caddy/Traefik) + forward-auth│ ──▶ tally web UI
                 │            ↕ (zero-trust)                     │
                 │     IdP: Authentik / Keycloak                │
   Claude/ChatGPT│   - forward-auth for the web UI              │
   MCP client ──▶│   - OAuth 2.1 authorization server for /mcp  │
                 └─────────────────────────────────────────────┘
                                   │ issues JWT
                                   ▼
                         tally  =  OAuth *resource server*
                         (validates JWT via JWKS; never issues tokens)
```

Three layers, each owned by the right component:

1. **Web-UI login** → `tower-sessions` + `axum-login` (sessions, `login_required` guards). Credentials still checked with `argon2`; 2FA still `totp-rs`. *(Phase 2)*
2. **MCP connector auth** → tally stops being an OAuth server. Per the **2025-06-18 MCP authorization spec**, the MCP server is a *resource server* that validates tokens minted by an external authorization server (the IdP) and advertises it via RFC 9728 Protected Resource Metadata. *(Phase 3)*
3. **Zero-trust on top** → an IdP/forward-auth in front of the web UI regardless, for defence-in-depth. *(Phase 4)*

## Crate decisions

| Concern | Crate | Version | Notes |
|---|---|---|---|
| Sessions | `tower-sessions` + `tower-sessions-sqlx-store` | latest 0.x | sqlite-backed store; replaces the `sessions` table |
| Login/guards | `axum-login` | latest 0.x | `AuthnBackend` + `login_required!`; same author as tower-sessions |
| Password hash | `argon2` | (existing) | keep — used inside the `AuthnBackend` |
| TOTP 2FA | `totp-rs` | (existing) | keep — partial-auth step after first factor |
| JWT validation | `jsonwebtoken` | latest 9.x | verify IdP-issued access tokens on `/mcp` |
| JWKS fetch/cache | `reqwest` (existing) + small cache | — | fetch IdP JWKS, cache by `kid`, refresh on miss |

**Rejected:** `oxide-auth-axum` (the only Rust OAuth2 *server* lib) — it would keep us in the business of *being* an authorization server. Delegating to the IdP is the spec-correct, lower-risk path. Documented here so the choice is explicit.

## Phase plan

- **Phase 1 — Rate limiting** ✅ *done.* `ratelimit.rs`: per-IP failure counter + exponential lockout on `login`, `verify-2fa`, `recovery`, `oauth/authorize`. Self-contained, unit-tested.
- **Phase 2 — Sessions/guards → axum-login.** Add deps; implement `Backend: AuthnBackend` over `Db` (username+Argon2id verify, then a TOTP partial-auth gate stored in the session); `AuthUser` for the user type; swap the custom `sessions` table for the tower-sessions store (migration); replace `middleware::require_auth` with `login_required`; **keep** the `Authorization: Bearer <api_token>` path for programmatic clients. Update the SPA's `login.tsx`/`setup.tsx`/`api.ts` to the new flow.
- **Phase 3 — Drop the OAuth server; `/mcp` becomes a resource server.** Delete `routes/oauth.rs` issuance + the consent page; drop `oauth_clients`/`oauth_access_tokens`/`auth_codes` tables (migration). `/mcp` validates `Bearer` JWTs against the IdP JWKS (`iss`/`aud`/`exp`/`scope`). Rewrite `/.well-known/oauth-protected-resource` to point `authorization_servers` at the configured IdP. New env: `TALLY_OIDC_ISSUER`, `TALLY_OIDC_AUDIENCE`, `TALLY_OIDC_JWKS_URL`. **Enforce scope per MCP tool** (read vs write) — fixes the audit's scope finding.
- **Phase 4 — Deploy stack + headers.** `docker-compose.idp.yml` (Authentik *recommended*, Keycloak alt) + reverse proxy with forward-auth; security-headers layer (CSP/HSTS/X-Frame-Options/nosniff/Referrer-Policy); fix the compose build-context bug; `SECURITY.md`; Dependabot + `cargo audit` CI.
- **Phase 5 — Deps/Docker hardening + re-audit.** `sqlx` `default-features=false` (drops the `rsa` advisory); digest-pinned images; compose `cap_drop`/`read_only`/`no-new-privileges`; remaining medium/low items (TOTP single-use replay guard, 12-char password floor, session rotation on 2FA, CSV body limit, Yahoo symbol encoding, AI plaintext fallback). Re-run the fan-out audit + `gitleaks` + `cargo audit` → confirm GO.

## Decisions taken (overridable)

- **IdP = Authentik** by default (single container, friendly admin UI, first-class forward-auth + OAuth2 provider). Keycloak documented as the enterprise alternative.
- **Backward-compat:** the existing `api_tokens` Bearer path stays during/after Phase 3 so local scripts / Home Assistant don't break; OIDC JWTs become the *recommended* path for the Claude/ChatGPT connector.
- **Migrations are additive + forward-only;** dropping OAuth tables happens in a dedicated, reversible-by-restore migration after the resource-server path is proven.

## Risks / call-outs

- **Deployment model changes:** self-hosters now run an IdP for the connector. Mitigated by shipping a ready compose file and keeping the Bearer-token path for the "just me, no IdP" case.
- **Frontend churn:** the SPA login changes in Phase 2 (and again if web UI moves fully behind forward-auth). Tracked with the backend changes.
- **No live IdP in CI:** JWKS validation is unit-tested with a local keypair; end-to-end is validated against the shipped Authentik compose.
