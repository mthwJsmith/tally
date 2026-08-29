# tally

Self-hosted personal finance for UK Open Banking. Rust (axum) backend, React frontend, one
Docker container, SQLite storage with secrets encrypted at rest. It includes an MCP server so AI
assistants can read your finances in near real-time, and built-in 2FA.

**[You can one-click deploy it for free to Google Cloud →](#deploy-to-google-cloud-run-one-click)**

## What it does

- Syncs UK bank accounts through TrueLayer. Multi-bank with a single OAuth callback URL.
- Stores every transaction locally in SQLite, not just references.
- Rules engine (regex to category), budgets, bills, and search/aggregate APIs.
- Tracks investment holdings and net worth.
- Custom recurring reminders and checklists (e.g. Help to Save, card due dates) with Telegram
  alerts, shown alongside your auto-detected direct debits.
- Exposes an MCP server so Claude, ChatGPT, and similar clients can query your data and read or
  tick reminders.
- Has its own auth: first-run setup, Argon2id passwords, and TOTP 2FA with recovery codes.

## Screenshots

All data below is fake demo data.

![Dashboard](.github/screenshots/dashboard.png)

| Transactions | Investments |
|---|---|
| ![Transactions](.github/screenshots/transactions.png) | ![Investments](.github/screenshots/investments.png) |

| Reminders and direct debits | |
|---|---|
| ![Reminders](.github/screenshots/reminders.png) | |

## Before you expose it publicly

tally aggregates your bank data, so treat it like a sensitive internal app. tally has its own
login and TOTP 2FA, but if it will be reachable from the internet we strongly recommend a
zero-trust access layer in front of it as well. The easiest option is Cloudflare Zero Trust
(Cloudflare Access) on your domain; an identity provider with forward-auth (Authentik, Authelia,
Keycloak) works the same way.

Gate the web UI, and leave only the paths that machines need reachable: `/mcp`,
`/.well-known/*`, and `/auth/callback` (the bank redirect). tally protects `/mcp` itself by
validating OIDC tokens, so bypassing the access layer on those paths does not leave them open.
If Claude or another MCP client connects but never shows any tally tools, this is the first
thing to check: the access layer is almost certainly intercepting `/mcp` or `/.well-known/*`
with a login redirect that the client cannot follow.

You do not have to rely on that layer alone. tally also protects itself: Argon2id password
hashing, TOTP 2FA with single-use codes, per-IP login rate limiting with lockout, ChaCha20-Poly1305
encryption of tokens at rest, and standard security headers. Run it behind a TLS reverse proxy and
set `TALLY_SECURE_COOKIES=true`.

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

## Deploy to Google Cloud Run (one-click)

[![Run on Google Cloud](https://deploy.cloud.run/button.svg)](https://console.cloud.google.com/cloudshell/editor?shellonly=true&cloudshell_image=gcr.io/cloudrun/button&cloudshell_git_repo=https://github.com/mthwjsmith/tally.git)

Builds and deploys tally to your own Google Cloud account, entirely in your browser — nothing to
install.

### What happens when you click it

1. It opens Google Cloud Shell and clones the repo. If it shows the **wrong Google account**, open
   the link in an **Incognito window** and sign in with the right one (Google has no per-link account
   switcher).
2. **Pick a project** — use the arrow keys to choose one of your existing projects, then press Enter.
   (Don't press Ctrl-C — that starts a slower new-project flow.) If it asks about **billing**, that's
   a one-time card setup: Cloud Run's free tier still applies, so you won't be charged for tally.
3. **Pick a region** — choose one near you, e.g. `europe-west2` (London).
4. It then asks for each setting below.

### What it asks you — when to just press Enter

You only need to **type** two things (your TrueLayer credentials). For everything else, **press Enter**
to accept the default.

| Prompt | What to do |
|--------|------------|
| `TALLY_MASTER_KEY` | **Press Enter** — a secure key is generated for you automatically. |
| `TALLY_TRUELAYER_CLIENT_ID` | **Type** your TrueLayer Live client id. |
| `TALLY_TRUELAYER_CLIENT_SECRET` | **Type** your TrueLayer Live client secret. |
| `TALLY_DATABASE_URL` | **Press Enter** for a throwaway test database — **or** paste a Turso URL to keep your data (most people want this; see below). |
| `TALLY_ENABLE_REMINDERS` | **Press Enter** (stays off — a serverless host can't run the background loop). |
| `TALLY_SECURE_COOKIES` | **Press Enter** (stays `true`). |
| `TALLY_TRUST_PROXY` | **Press Enter** (stays `true`). |

Then it builds (~5–10 min) and prints your `https://…run.app` URL. Open it, create your user, enrol 2FA.

### Keep your data: a free Turso database (recommended for real use)

The default database lives on Cloud Run's disk, which is **wiped every time the service goes idle and
restarts** — fine to *try* tally, but you'd lose your account and bank links. To keep everything,
point tally at a free [Turso](https://turso.tech) database (hosted SQLite — same engine tally already
uses):

1. Sign up at [turso.tech](https://turso.tech) — the free tier is plenty for one person.
2. Create a database and copy its URL — it looks like `libsql://your-db-name.turso.io`.
3. Create a database **auth token** and copy it.
4. At the **`TALLY_DATABASE_URL`** prompt during deploy, paste the `libsql://…` URL (instead of
   pressing Enter).
5. After the deploy finishes, add the token: **Cloud Run → your service → Edit & deploy new revision →
   Variables & Secrets → add `TALLY_DATABASE_AUTH_TOKEN` = your token → Deploy.**

Now your data survives restarts.

### Link a bank: set the redirect URL

Before the "add bank" button works, TrueLayer needs to know where to send you back after login:

1. Copy your `https://YOUR-SERVICE.run.app` URL.
2. **Cloud Run → your service → Edit & deploy new revision → Variables → add
   `TALLY_REDIRECT_URI_BASE` = `https://YOUR-SERVICE.run.app/auth` → Deploy.**
3. In the TrueLayer console, add `https://YOUR-SERVICE.run.app/auth/callback` to your app's allowed
   redirect URIs.

### Lock it down with Identity-Aware Proxy (recommended)

The button deploys the service publicly — tally's own Argon2 + 2FA login still gates it, but for a
stronger Google-sign-in gate, enable **IAP**: one click, no load balancer, no extra cost. **Cloud Run
→ your service → Security → enable Identity-Aware Proxy**, then grant yourself the **IAP-secured Web
App User** role. (IAP gates the human web UI; it is *not* compatible with the headless `/mcp` AI
connector — on an IAP deployment leave `/mcp` disabled, or front it with Cloudflare Access instead.)

## Configuration

Full list is in `apps/api/.env.example`. The ones you usually need:

| Variable | Purpose |
|----------|---------|
| `TALLY_MASTER_KEY` | 32-byte base64 key that encrypts tokens at rest (required) |
| `TALLY_TRUELAYER_CLIENT_ID` / `_SECRET` | TrueLayer Live credentials (required) |
| `TALLY_DATABASE_URL` | `file:/path/state.db` (default, local) or `libsql://…turso.io` for hosted Turso (then set `TALLY_DATABASE_AUTH_TOKEN`) |
| `TALLY_ENABLE_REMINDERS` | `true` (default) runs reminders/Telegram/sync; set `false` on serverless |
| `TALLY_REDIRECT_URI_BASE` | Public base URL TrueLayer redirects back to |
| `TALLY_OIDC_ISSUER` / `TALLY_OIDC_AUDIENCE` | External OIDC provider for the MCP connector |
| `TALLY_SECURE_COOKIES` | `true` (default) marks session cookies HTTPS-only; set `false` only for plain-HTTP access (e.g. raw LAN IP — `http://localhost` works regardless) |
| `TALLY_TRUST_PROXY` | Set to `true` only behind a trusted reverse proxy (rate-limit keys then use `X-Forwarded-For`) |

## MCP connector

`/mcp` accepts access tokens issued by your OIDC provider and validates them against the
provider's JWKS (signature, issuer, audience, expiry). Read tools accept any valid token; write
tools (recording an investment, adding or ticking a reminder) require a token with the `write`
scope. There are two ways to provide the OIDC side.

### Option A: Cloudflare Access Managed OAuth (nothing extra to run)

If tally already sits behind Cloudflare, you do not need to host your own identity provider.
In Cloudflare Zero Trust, create a self-hosted Access application covering your tally hostname's
`/mcp` path, enable **Managed OAuth** in its advanced settings, add an Allow policy for yourself,
and copy the application's AUD tag. Then set:

```
TALLY_OIDC_ISSUER=https://<your-team>.cloudflareaccess.com
TALLY_OIDC_AUDIENCE=<the Access application AUD tag>
TALLY_OIDC_JWKS_URL=https://<your-team>.cloudflareaccess.com/cdn-cgi/access/certs
```

Cloudflare runs the OAuth flow for claude.ai and forwards each request with a
`Cf-Access-Jwt-Assertion` JWT, which tally validates like any other OIDC token. Cloudflare's
tokens carry no `write` scope, so this path is read-only.

### Option B: your own OIDC provider

Create an OAuth2/OIDC application in your provider (Authentik, Keycloak, Auth0, and so on), set
`TALLY_OIDC_ISSUER` and `TALLY_OIDC_AUDIENCE` to match, then add the connector in your AI client.
A ready-to-run Authentik stack is included (note: it is too heavy for small hosts like a 2 GB Pi;
run it on a bigger box or use Option A):

```bash
docker compose -f docker-compose.yml -f docker-compose.idp.yml up -d
```

## Reminders

Reminders are recurring checklist items (hourly, daily, weekly, or monthly on a fixed day of the
month). tally pings you on Telegram when one is due and not ticked, then rolls it to the next
period. The screen also shows your auto-detected direct debits, so it is one place for everything
you owe or need to do.

## Build from source

- Backend: Rust stable, then `cargo build --release` in `apps/api`.
- Frontend: Node 22, then `npm install` and `npm run build` in `apps/web`.

The Dockerfile builds both stages and bundles the built SPA into the binary's static directory.

## Security reporting

See `SECURITY.md`. Use private vulnerability reporting rather than public issues.

## License

MIT. See `LICENSE`.
