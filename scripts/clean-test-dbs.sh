#!/usr/bin/env bash
# Drops the throwaway databases created by the integration tests.
set -euo pipefail
PSQL=${PSQL:-"docker exec wikicraft-pg psql -U wikicraft -d postgres"}
for db in $($PSQL -tAc "SELECT datname FROM pg_database WHERE datname LIKE 'wikicraft_test_%'"); do
  $PSQL -qc "DROP DATABASE \"$db\""
done
echo "done"
