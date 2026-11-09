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
# SAML support (`--build-arg FEATURES=saml`, the default) links libxmlsec1 for XML signatures
ARG FEATURES=saml
RUN if [ -n "$FEATURES" ]; then apt-get update && apt-get install -y --no-install-recommends libxml2-dev libxmlsec1-dev libclang-dev clang pkg-config && rm -rf /var/lib/apt/lists/*; fi
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p wikicraft --features "$FEATURES" && cp target/release/wikicraft /wikicraft

# ---- runtime ----
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libxml2 libxmlsec1 libxmlsec1-openssl && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /data wikicraft && mkdir -p /data/blobs /data/uploads /data/index && chown -R wikicraft /data
COPY --from=server /wikicraft /usr/local/bin/wikicraft
COPY --from=web /web/dist /srv/wikicraft/web
USER wikicraft
ENV WIKICRAFT_BIND=0.0.0.0:8787 \
    WIKICRAFT_DATA_DIR=/data \
    WIKICRAFT_STATIC_DIR=/srv/wikicraft/web
EXPOSE 8787
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s CMD ["wikicraft", "healthcheck"]
ENTRYPOINT ["wikicraft"]
CMD ["serve"]
