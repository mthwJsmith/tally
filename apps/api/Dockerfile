# Multi-stage build for tally — bundles React SPA into the Rust binary's static-serve dir.
#
# Build context: the monorepo ROOT (tally/), NOT apps/api/.
# Invoke from monorepo root:  docker build -f apps/api/Dockerfile -t tally:local .
#
# ARM64 native build on a Pi 5 takes ~6-10 min first time; ~2-3 min on subsequent rebuilds
# with cargo-chef warm cache.

# ---- frontend build -------------------------------------------------------
FROM node:22-slim@sha256:7af03b14a13c8cdd38e45058fd957bf00a72bbe17feac43b1c15a689c029c732 AS web-build
WORKDIR /web
COPY apps/web/package.json apps/web/package-lock.json* ./
RUN npm install --no-audit --no-fund --silent
COPY apps/web/ ./
RUN npm run build
# → /web/dist

# ---- planner: extract Rust dep recipe -------------------------------------
FROM rust:1.97-slim-bookworm@sha256:6d220bf85c74e842a79da63997af8d2e74455c0b8847d8bb3a5888572334991d AS planner
WORKDIR /app
RUN cargo install cargo-chef --locked
COPY apps/api/Cargo.toml apps/api/Cargo.lock ./
COPY apps/api/src ./src
COPY apps/api/migrations ./migrations
COPY apps/api/templates ./templates
COPY apps/api/static ./static
RUN cargo chef prepare --recipe-path recipe.json

# ---- builder: deps then app ----------------------------------------------
FROM rust:1.97-slim-bookworm@sha256:6d220bf85c74e842a79da63997af8d2e74455c0b8847d8bb3a5888572334991d AS builder
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
ENV SQLX_OFFLINE=true
# The Pi 5 has only ~2GB RAM. rustc's default parallel codegen spikes memory hard enough to
# invoke the OOM-killer (it has taken out cloudflared mid-build). Serialise compilation (one job)
# so peak memory stays bounded — slower, but the dep layers are cached so only the app crate
# rebuilds. Keep default codegen-units (many small units) so each serial unit stays small.
ENV CARGO_BUILD_JOBS=1
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
# Bundle the built SPA into the static-serve directory.
COPY --from=web-build /web/dist /app/web

RUN mkdir -p /app/data && chown -R tally:tally /app
USER tally
EXPOSE 3001
HEALTHCHECK --interval=60s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -fsS http://localhost:3001/healthz || exit 1
CMD ["tally"]
