# justdeploy: the Rust control plane, serving the built dashboard.

FROM node:24-alpine AS web
WORKDIR /web
RUN npm install -g pnpm@12.6.0
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1-bookworm AS api
WORKDIR /src
COPY templates templates
COPY api/Cargo.toml api/Cargo.lock api/
COPY api/.sqlx api/.sqlx
COPY api/migrations api/migrations
COPY api/src api/src
# Queries are checked against the committed .sqlx cache; no database at build time.
ENV SQLX_OFFLINE=true
RUN cd api && cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=api /src/api/target/release/justdeploy /usr/local/bin/justdeploy
COPY --from=web /web/dist /app/web
ENV JD_WEB_DIR=/app/web \
    JD_BIND=0.0.0.0:8080 \
    JD_BACKUP_DIR=/backups
EXPOSE 8080
CMD ["justdeploy"]
