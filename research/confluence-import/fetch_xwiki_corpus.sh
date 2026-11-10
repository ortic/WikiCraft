#!/usr/bin/env bash
# Downloads the XWiki Confluence importer's test corpus (github.com/xwiki-contrib/confluence,
# LGPL-2.1) into ./xwiki-corpus/: ~90 small, targeted Confluence exports (broken links, buggy
# history, missing parents, orphans, inline comments, content statuses, Cloud CSV backups, ...).
# Used as edge-case regression tests by crates/confluence/tests/xwiki_corpus.rs; downloaded, not
# vendored. Pinned to a commit so the expectations in the test stay valid.
set -euo pipefail
cd "$(dirname "$0")"
COMMIT=e9daf2a275868b87fa5575dc2a214b26a721dadd
DEST=xwiki-corpus
if [ -f "$DEST/.commit" ] && [ "$(cat "$DEST/.commit")" = "$COMMIT" ]; then
  echo "xwiki corpus already at $COMMIT"
  exit 0
fi
rm -rf "$DEST" "$DEST.tmp"
git init -q "$DEST.tmp"
git -C "$DEST.tmp" remote add origin https://github.com/xwiki-contrib/confluence.git
git -C "$DEST.tmp" config core.sparseCheckout true
printf '/LICENSE\n/confluence-xml/src/test/resources/confluencexml/\n' > "$DEST.tmp/.git/info/sparse-checkout"
git -C "$DEST.tmp" fetch -q --depth 1 --filter=blob:none origin "$COMMIT"
git -C "$DEST.tmp" checkout -q FETCH_HEAD
mkdir -p "$DEST"
mv "$DEST.tmp/LICENSE" "$DEST/LICENSE"
mv "$DEST.tmp/confluence-xml/src/test/resources/confluencexml" "$DEST/cases"
rm -rf "$DEST.tmp"
echo "$COMMIT" > "$DEST/.commit"
echo "xwiki corpus: $(ls "$DEST/cases" | wc -l) entries in $DEST/cases"
