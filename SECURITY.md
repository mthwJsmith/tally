# Security Policy

tally aggregates sensitive personal financial data. Security reports are taken seriously.

## Reporting a vulnerability

**Please do not open a public issue for security problems.**

Use GitHub's **[Private vulnerability reporting](https://github.com/mthwjsmith/tally/security/advisories/new)**
(Security tab → "Report a vulnerability"). Include:

- affected version / commit,
- a description and impact assessment,
- reproduction steps or a proof of concept.

You can expect an acknowledgement within **7 days** and a fix or mitigation timeline after triage.
Please allow a reasonable disclosure window before going public.

## Scope

In scope: authentication/session handling, the MCP token validation path, OAuth/OIDC
integration, secret-at-rest encryption, injection, and anything that exposes another user's or
the operator's financial data.

Out of scope: issues that require a pre-compromised host, missing hardening on a deployment that
ignores the documented reverse-proxy/IdP guidance, and findings solely in third-party
dependencies (report those upstream, though we welcome a heads-up).

## Deployment hardening (operator responsibilities)

tally is self-hosted; a secure deployment depends on the operator:

- Put it behind a **TLS-terminating reverse proxy** and set `TALLY_SECURE_COOKIES=true`.
- For internet exposure, front the web UI with a zero-trust layer (Cloudflare Access) or an
  **IdP / forward-auth** (Authentik, Authelia, Keycloak). See the README's security section.
- Keep `TALLY_MASTER_KEY` and `.env` secret and backed up; never commit them.
- Treat API tokens and IdP credentials as full-access secrets.

## What the app does for you

- Argon2id password hashing; TOTP 2FA with encrypted recovery codes.
- Sessions via `tower-sessions` + `axum-login` (no hand-rolled session tokens).
- Per-IP rate limiting + progressive lockout on auth endpoints.
- ChaCha20-Poly1305 encryption of bank/OAuth tokens at rest.
- `/mcp` validates externally-issued OIDC JWTs (resource-server model); it issues no tokens.
- Security headers (X-Frame-Options, nosniff, Referrer-Policy, CSP `frame-ancestors`, HSTS).
