#!/usr/bin/env bash
# Downloads the public Confluence exports used for the import dry-run into ./fixtures/.
# These repos carry NO license: use locally for testing only, never commit or redistribute them.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p fixtures/lf-toip-cloud-csv fixtures/modelop-cloud-xml

# 1) Trust over IP Foundation wiki (lf-toip.atlassian.net, space HOME) — Confluence Cloud CSV export, Oct 2026
base=https://raw.githubusercontent.com/LFDT-way/mcp-cache/HEAD/confluence/lf-toip.atlassian.net
for f in bodycontent.csv.gz content.csv.gz spaces.csv.gz exportDescriptor.properties content-sha256.tsv; do
  curl -sSfL -o "fixtures/lf-toip-cloud-csv/$f" "$base/HOME-CSV/$f"
done
curl -sSfL -o fixtures/lf-toip-cloud-csv/export-users.csv "$base/export-users.csv"

# 2) ModelOp docs space (DV21) — Confluence Cloud classic XML export (entities.xml + attachments), Sep 2020
curl -sSfL -o fixtures/modelop-cloud-xml/export.zip \
  "https://raw.githubusercontent.com/modelop/docs-export/HEAD/Confluence-export%20(6).zip"

echo "Done. Run:"
echo "  python3 -I analyze_confluence.py csv fixtures/lf-toip-cloud-csv"
echo "  python3 -I analyze_confluence.py xml fixtures/modelop-cloud-xml/export.zip"
