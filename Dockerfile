# syntax=docker/dockerfile:1.7
# ---- frontend ----
FROM node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# ---- backend ----
FROM rust:1.96-bookworm AS server
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p wikicraft && cp target/release/wikicraft /wikicraft

# ---- runtime ----
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /data wikicraft && mkdir -p /data && chown wikicraft /data
COPY --from=server /wikicraft /usr/local/bin/wikicraft
COPY --from=web /web/dist /srv/wikicraft/web
USER wikicraft
ENV WIKICRAFT_BIND=0.0.0.0:8787 \
    WIKICRAFT_DATA_DIR=/data \
    WIKICRAFT_STATIC_DIR=/srv/wikicraft/web
EXPOSE 8787
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=3s CMD ["/bin/sh", "-c", "exec 3<>/dev/tcp/127.0.0.1/8787 && printf 'GET /api/health HTTP/1.0\\r\\n\\r\\n' >&3 && grep -q '\"ok\"' <&3"]
ENTRYPOINT ["wikicraft"]
CMD ["serve"]
