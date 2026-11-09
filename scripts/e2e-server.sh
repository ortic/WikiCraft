#!/usr/bin/env bash
# Starts WikiCraft for the end-to-end tests on a fresh database (wikicraft_e2e).
# Needs Postgres (WIKICRAFT_E2E_ADMIN_URL, default: the dev container on 127.0.0.1:55432)
# and a built frontend (web/dist).
set -euo pipefail
cd "$(dirname "$0")/.."
PORT="${E2E_PORT:-8799}"
ADMIN_URL="${WIKICRAFT_E2E_ADMIN_URL:-postgres://wikicraft:wikicraft@127.0.0.1:55432/postgres}"
DB_URL="${ADMIN_URL%/*}/wikicraft_e2e"
sql() {
  if command -v psql >/dev/null; then psql "$ADMIN_URL" -qc "$1"
  else docker exec wikicraft-pg psql -U wikicraft -d postgres -qc "$1"; fi
}
sql "DROP DATABASE IF EXISTS wikicraft_e2e WITH (FORCE)"
sql "CREATE DATABASE wikicraft_e2e"
[ -f web/dist/index.html ] || (cd web && npm ci && npm run build)
DATA="$(mktemp -d)"
trap 'rm -rf "$DATA"' EXIT
exec env WIKICRAFT_DATABASE_URL="$DB_URL" WIKICRAFT_BIND="127.0.0.1:$PORT" WIKICRAFT_BASE_URL="http://127.0.0.1:$PORT" \
  WIKICRAFT_DATA_DIR="$DATA" WIKICRAFT_STATIC_DIR=web/dist RUST_LOG="${RUST_LOG:-warn}" \
  cargo run --release -q -p wikicraft -- serve
