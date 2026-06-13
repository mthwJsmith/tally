# Root Dockerfile — used by the "Run on Google Cloud" button (Cloud Run Button builds the
# Dockerfile found at the repo root). It is identical to apps/api/Dockerfile, which already
# builds with the repo root as context. KEEP THE TWO IN SYNC: the Pi / docker-compose build
# uses apps/api/Dockerfile; this one exists only so the one-click Cloud Run deploy can find a
# root Dockerfile. Multi-stage build that bundles the React SPA into the Rust binary.

# ---- frontend build -------------------------------------------------------
FROM node:22-slim@sha256:7af03b14a13c8cdd38e45058fd957bf00a72bbe17feac43b1c15a689c029c732 AS web-build
WORKDIR /web
COPY apps/web/package.json apps/web/package-lock.json* ./
RUN npm install --no-audit --no-fund --silent
COPY apps/web/ ./
RUN npm run build
# → /web/dist

# ---- planner: extract Rust dep recipe -------------------------------------
FROM rust:1.89-slim-bookworm@sha256:d7fc7de78bb8c1469933aeecbf801314d30d7d6e9f0578bba4cfa285bfa37fe6 AS planner
WORKDIR /app
RUN cargo install cargo-chef --locked
COPY apps/api/Cargo.toml apps/api/Cargo.lock ./
COPY apps/api/src ./src
COPY apps/api/migrations ./migrations
COPY apps/api/templates ./templates
COPY apps/api/static ./static
RUN cargo chef prepare --recipe-path recipe.json

# ---- builder: deps then app ----------------------------------------------
FROM rust:1.89-slim-bookworm@sha256:d7fc7de78bb8c1469933aeecbf801314d30d7d6e9f0578bba4cfa285bfa37fe6 AS builder
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config build-essential ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY apps/api/Cargo.toml apps/api/Cargo.lock ./
COPY apps/api/src ./src
COPY apps/api/migrations ./migrations
COPY apps/api/templates ./templates
COPY apps/api/static ./static
RUN cargo build --release --bin tally
RUN strip target/release/tally

# ---- runtime --------------------------------------------------------------
FROM debian:bookworm-slim@sha256:0104b334637a5f19aa9c983a91b54c89887c0984081f2068983107a6f6c21eeb AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 tally \
    && useradd --uid 1000 --gid tally --shell /bin/bash --create-home tally

WORKDIR /app
COPY --from=builder /app/target/release/tally /usr/local/bin/tally
COPY --from=builder /app/migrations /app/migrations
COPY --from=builder /app/templates /app/templates
COPY --from=builder /app/static /app/static
COPY --from=web-build /web/dist /app/web

RUN mkdir -p /app/data && chown -R tally:tally /app
USER tally
EXPOSE 3001
HEALTHCHECK --interval=60s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -fsS http://localhost:3001/healthz || exit 1
CMD ["tally"]
