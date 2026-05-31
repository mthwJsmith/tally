# tally

**Self-hosted personal finance OS. Multi-bank UK Open Banking sync (TrueLayer + Plaid) →
SQLite → TanStack web UI. Built-in MCP server. Built-in 2FA.**

Rust backend (Axum + sqlx + ChaCha20-Poly1305 encrypted secrets at rest) bundled with a React
+ TanStack + shadcn frontend in a single static-binary Docker image. Designed for the Pi 5 era —
single container, single port, ~15 MB idle RAM, ~25 MB image.

## Why it exists

Existing options in the UK Open Banking + self-host space (May 2026):
- **truelayer2firefly** (upstream) — Python, single-bank only, plaintext tokens, ignores
  standing orders / direct debits, crashes on `None` IBAN. We audited it, decided to rewrite.
- **Firefly III** — mature but AGPL, PHP/Laravel, ~150 MB idle, UI feels dated. Great features
  but heavy for a Pi-5.
- **Enable Banking** — clean API but EU/Nordic only, no UK coverage.
- **GoCardless Bank Account Data** — closed to new personal signups since July 2025.

So tally:
1. Uses TrueLayer's free Live tier for UK personal use (Plaid optional, soon).
2. Encrypts OAuth tokens at rest (ChaCha20-Poly1305).
3. Supports multi-bank — one OAuth callback URL, nickname carried in state token.
4. Stores all transactions in its own SQLite (not just IDs).
5. Has a rules engine (regex → category), budgets, bills, search/filter/aggregate APIs.
6. Built-in MCP server so Claude / Gemini / ChatGPT can query natively.
7. Built-in Argon2 + TOTP 2FA auth.
8. Modern React + TanStack UI (Table virtualization, faceted filters, dashboard, charts).
9. Single docker container, MIT-licensed, builds for ARM64.

## Structure

```
tally/
├── README.md
├── LICENSE                  # MIT
├── docker-compose.yml       # self-host one-liner
├── apps/
│   ├── api/                 # Rust backend (Axum + sqlx)
│   │   ├── src/
│   │   ├── migrations/
│   │   ├── Cargo.toml
│   │   └── Dockerfile       # multi-stage; final stage embeds web/dist
│   └── web/                 # React frontend (Vite + TanStack + shadcn)
│       ├── src/
│       └── package.json
└── docs/
    ├── ARCHITECTURE.md
    ├── DEPLOY.md
    ├── PROVIDERS.md
    └── MCP.md
```

## Quickstart (self-host)

```bash
git clone https://github.com/mthwjsmith/tally.git
cd tally
cp apps/api/.env.example .env
# generate TALLY_MASTER_KEY:   openssl rand -base64 32
# (no Firefly III needed if you use tally's own UI; leave TALLY_FIREFLY_URL empty)
# fill in TrueLayer client_id + secret from console.truelayer.com (Live mode)

docker compose up -d --build
# open https://localhost:3001
# register first user, enrol 2FA, add your first bank via OAuth, wait an hour
```

## Status (May 2026)

- ✅ v1 sync pipe — multi-bank TrueLayer → Firefly III; production-ready
- 🚧 v2 backend — full transaction storage, rules, budgets, bills, MCP, auth, Plaid
- 🚧 v3 frontend — TanStack-driven SPA
- 🔜 v4 — Firefly III decommissioned from the host

## License

MIT — copy / adapt freely. Do not paste in AGPL code (e.g., from Firefly III); keep this MIT.
