// Fix for @tiptap/y-tiptap (3.2.0 and 3.3.0): after someone else's change, the cursor goes back
// to where Yjs says it belongs, unless a "structural change" heuristic thinks the position was
// misresolved. That heuristic counts any text change in the cursor's paragraph as structural,
// so typing at the end of a paragraph while someone types earlier in it put the cursor back
// before your own last letter ("beta" arrived as "ateb"). The patch trusts the Yjs position
// whenever the character it is anchored to (and every text and block around it) still exists;
// the heuristic still handles real structural changes (a paragraph replaced or split).
//
// Applied to the package's code when Vite loads it (dev server and builds). The build fails if
// the code it replaces changes, so an upgrade can't silently drop or misapply the fix: check
// the new version (the e2e test "Chromium and Firefox edit one paragraph at once" covers it).

import type { Plugin } from 'vite'

const FILE = /@tiptap[\\/]y-tiptap[\\/]dist[\\/]index\.js$/

const REPLACEMENTS: [string, string][] = [
  [
    `const recoverSelectionEndpoint = (newDoc, oldDoc, oldAbs, resolved) => {
	if (oldAbs == null || oldAbs < 0 || oldAbs > oldDoc.content.size) return resolved;`,
    `const wcPositionIntact = (relPos, ydoc) => {
	if (relPos == null || relPos.item == null || ydoc == null) return false;
	let item;
	try {
		item = Y.getItem(ydoc.store, relPos.item);
	} catch {
		return false;
	}
	if (!(item instanceof Y.Item) || item.deleted) return false;
	for (let t = item.parent; t != null && t._item != null; t = t._item.parent) if (t._item.deleted) return false;
	return true;
};
const recoverSelectionEndpoint = (newDoc, oldDoc, oldAbs, resolved, relPos, ydoc) => {
	if (oldAbs == null || oldAbs < 0 || oldAbs > oldDoc.content.size) return resolved;
	if (resolved !== null && wcPositionIntact(relPos, ydoc)) return resolved;`,
  ],
  [
    `anchor = recoverSelectionEndpoint(tr.doc, oldDoc, relSel.absAnchor, anchor);`,
    `anchor = recoverSelectionEndpoint(tr.doc, oldDoc, relSel.absAnchor, anchor, relSel.anchor, binding.doc);`,
  ],
  [
    `head = recoverSelectionEndpoint(tr.doc, oldDoc, relSel.absHead, head);`,
    `head = recoverSelectionEndpoint(tr.doc, oldDoc, relSel.absHead, head, relSel.head, binding.doc);`,
  ],
]

export function fixYTiptap(): Plugin {
  let patched = false
  return {
    name: 'wikicraft-fix-y-tiptap',
    enforce: 'pre',
    buildEnd(error) {
      // a production build that never saw the file would ship the bug
      if (!error && !this.meta.watchMode && !patched) this.error('@tiptap/y-tiptap was not patched (web/scripts/fix-y-tiptap.ts): was the file moved?')
    },
    transform(code, id) {
      if (!FILE.test(id.split('?')[0])) return null
      patched = true
      let out = code
      for (const [from, to] of REPLACEMENTS) {
        if (!out.includes(from)) throw new Error(`@tiptap/y-tiptap changed: the cursor fix in web/scripts/fix-y-tiptap.ts no longer applies (missing: ${from.split('\n')[0]})`)
        out = out.replace(from, to)
      }
      return { code: out, map: null }
    },
  }
}
