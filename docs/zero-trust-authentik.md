# Zero-trust + MCP auth with Authentik

tally delegates identity to an external provider. [Authentik](https://github.com/goauthentik/authentik)
plays two roles:

1. **MCP authorization server** — issues the JWT access tokens that Claude/ChatGPT present to
   `POST /mcp`. tally validates them via JWKS (it never issues tokens).
2. **Zero-trust forward-auth** — sits in front of the tally web UI so every browser request is
   authenticated by Authentik (password + TOTP + passkeys + lockout) before it reaches tally.

```
Browser ──▶ Reverse proxy (Caddy/Traefik) ──forward-auth──▶ Authentik ──▶ tally web UI
Claude  ──▶ /mcp  (Bearer JWT) ─────────────validate via JWKS────────────▶ tally resource server
```

## 1. Boot Authentik

Add to `.env`:

```
AUTHENTIK_SECRET_KEY=<openssl rand -base64 60>
PG_PASS=<openssl rand -base64 36>
```

```
docker compose -f docker-compose.yml -f docker-compose.idp.yml up -d
```

Open `http://localhost:9000/if/flow/initial-setup/` and create the admin account.

## 2. Create the MCP OAuth provider (token issuer for /mcp)

In Authentik admin → **Applications → Providers → Create → OAuth2/OpenID Provider**:

- **Name:** `tally-mcp`
- **Authorization flow:** default authorization (explicit consent is fine)
- **Client type:** Public (MCP clients use PKCE, no client secret)
- **Redirect URIs:** allow the MCP client callbacks (Claude/ChatGPT connector redirect URLs)
- **Signing key:** the default RSA key
- **Scopes:** `openid`, `profile`, plus a custom `read` (and optionally `write`) scope mapping

Then **Applications → Create**, name `tally`, and bind the provider above.

Note the provider's **issuer URL** (Authentik shows it under the provider, e.g.
`https://auth.example.com/application/o/tally/`). Set in tally's `.env`:

```
TALLY_OIDC_ISSUER=https://auth.example.com/application/o/tally/
TALLY_OIDC_AUDIENCE=tally-mcp          # must match the provider's client_id / audience
# TALLY_OIDC_JWKS_URL=                  # defaults to <issuer>/jwks/
```

Restart tally. On boot it logs `MCP OIDC resource server: issuer=…`. When an MCP client hits
`/mcp` without a token, tally returns `401` with a `WWW-Authenticate` header pointing at
`/.well-known/oauth-protected-resource`, whose `authorization_servers` now lists your Authentik
issuer — so the client discovers Authentik and runs the OAuth flow against it.

> `/mcp` accepts **only** OIDC JWTs — there is no API-token fallback. (API tokens still work for
> the `/api/*` REST endpoints used by scripts / Home Assistant; they just aren't accepted on `/mcp`.)

## 3. Put the web UI behind forward-auth (zero-trust)

Create a **Proxy Provider** (forward-auth, single application) in Authentik and an **Outpost**
(the embedded outpost is fine), then point your reverse proxy at it. Example Caddy:

```caddyfile
finance.example.com {
    # Endpoints that MUST stay public (programmatic clients / OAuth callback):
    @public path /mcp* /.well-known/* /healthz /auth/callback
    handle @public {
        reverse_proxy 127.0.0.1:3001
    }

    # Everything else (the web UI) goes through Authentik forward-auth:
    handle {
        forward_auth 127.0.0.1:9000 {
            uri /outpost.goauthentik.io/auth/caddy
            copy_headers X-Authentik-Username X-Authentik-Groups X-Authentik-Email
        }
        reverse_proxy 127.0.0.1:3001
    }
}
```

Set `TALLY_SECURE_COOKIES=true` in `.env` since the proxy terminates TLS.

> **Why the `@public` carve-out:** Claude/ChatGPT and the OAuth callback can't pass an
> interactive forward-auth login, so `/mcp`, `/.well-known/*`, and `/auth/callback` bypass it.
> They are protected by their own checks (JWT validation on `/mcp`; the single-use `state`
> token on `/auth/callback`).

## 4. Defence-in-depth still in the app

Even fully behind Authentik, tally keeps its own controls so it is never *solely* dependent on
the edge: built-in session auth + TOTP (for the no-IdP case), per-IP rate limiting/lockout on
auth endpoints, and the security headers layer. See `docs/auth-refactor.md`.
