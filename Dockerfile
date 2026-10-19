# syntax=docker/dockerfile:1

FROM node:22-slim AS web-builder
WORKDIR /build
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

FROM rust:1-bookworm AS rust-builder
WORKDIR /build
COPY app/Cargo.toml app/Cargo.lock ./
COPY app/src ./src
COPY app/migrations ./migrations
COPY --from=web-builder /build/dist /web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    cargo build --release \
    && cp target/release/auau /build/auau

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 1000 --home-dir /app auau \
    && mkdir -p /app/data && chown -R auau:auau /app/data
COPY --from=rust-builder /build/auau /usr/local/bin/auau
USER auau
WORKDIR /app
EXPOSE 8080
ENTRYPOINT ["auau"]
