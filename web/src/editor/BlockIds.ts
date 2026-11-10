// Stable block ids: every block (paragraph, heading, list item, table, callout, …) carries a
// short random `blockId`, so links can point at a block (`/pages/<id>#block-<blockId>`) and the
// id survives edits, moves and renames. The server assigns ids to older pages when they are
// opened (crates/doc-model/src/blocks.rs, same type list), so opening a page isn't an edit.

import { UniqueID } from '@tiptap/extension-unique-id'
import type { Node as PMNode } from '@tiptap/pm/model'
import type { EditorView } from '@tiptap/pm/view'
import shared from '../../../crates/doc-model/assets/block_types.json'

export const BLOCK_ID_ATTR: string = shared.attribute
export const BLOCK_TYPES: string[] = shared.types

const ALPHABET = 'abcdefghijklmnopqrstuvwxyz0123456789'

/** 8 characters, starting with a letter (same format as the server). */
export function newBlockId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(8))
  let id = ALPHABET[bytes[0] % 26]
  for (let i = 1; i < bytes.length; i++) id += ALPHABET[bytes[i] % ALPHABET.length]
  return id
}

export const BlockIds = UniqueID.configure({
  attributeName: BLOCK_ID_ATTR,
  types: BLOCK_TYPES,
  generateID: () => newBlockId(),
})

/** URL fragment for a block. */
export const blockHash = (blockId: string) => `#block-${blockId}`

/** Position of the block with `id`, or null. */
export function blockPos(doc: PMNode, id: string): number | null {
  let found: number | null = null
  doc.descendants((n, pos) => {
    if (found !== null) return false
    if (n.attrs?.[BLOCK_ID_ATTR] === id) {
      found = pos
      return false
    }
    return true
  })
  return found
}

const ITEMS = new Set(['listItem', 'taskItem', 'decisionItem'])

/**
 * The block a "copy link" button at (x, y) points at: the innermost block with an id, except
 * that the first paragraph of a list item stands for the item and anything inside a table for
 * the table.
 */
export function blockAt(view: EditorView, x: number, y: number): { id: string; pos: number } | null {
  const hit = view.posAtCoords({ left: x, top: y })
  if (!hit) return null
  const doc = view.state.doc
  if (hit.inside >= 0) {
    const atom = doc.nodeAt(hit.inside)
    if (atom?.isBlock && atom.attrs[BLOCK_ID_ATTR] && doc.resolve(hit.inside).depth === 0) return { id: atom.attrs[BLOCK_ID_ATTR], pos: hit.inside }
  }
  const $pos = doc.resolve(hit.inside >= 0 ? hit.inside : hit.pos)
  for (let d = 1; d <= $pos.depth; d++) {
    if ($pos.node(d).type.name === 'table' && $pos.node(d).attrs[BLOCK_ID_ATTR]) return { id: $pos.node(d).attrs[BLOCK_ID_ATTR], pos: $pos.before(d) }
  }
  for (let d = $pos.depth; d >= 1; d--) {
    const node = $pos.node(d)
    const id = node.attrs[BLOCK_ID_ATTR] as string | null | undefined
    if (!id) continue
    const parent = d > 1 ? $pos.node(d - 1) : null
    if (parent && ITEMS.has(parent.type.name) && parent.attrs[BLOCK_ID_ATTR] && $pos.index(d - 1) === 0) {
      return { id: parent.attrs[BLOCK_ID_ATTR], pos: $pos.before(d - 1) }
    }
    return { id, pos: $pos.before(d) }
  }
  if (hit.inside >= 0) {
    const atom = doc.nodeAt(hit.inside)
    if (atom?.attrs[BLOCK_ID_ATTR]) return { id: atom.attrs[BLOCK_ID_ATTR], pos: hit.inside }
  }
  return null
}

/** The element for `#block-<id>` or a heading anchor `#<slug>`. */
export function findAnchor(view: EditorView, hash: string): HTMLElement | null {
  const h = decodeURIComponent(hash.replace(/^#/, ''))
  if (!h) return null
  if (h.startsWith('block-')) {
    const pos = blockPos(view.state.doc, h.slice(6))
    const dom = pos === null ? null : view.nodeDOM(pos)
    return dom instanceof HTMLElement ? dom : null
  }
  return view.dom.querySelector<HTMLElement>(`[id="${CSS.escape(h)}"]`)
}

/** Scrolls to the block of `hash` and flashes it; false when it isn't on the page. */
export function revealAnchor(view: EditorView, hash: string): boolean {
  const el = findAnchor(view, hash)
  if (!el) return false
  el.scrollIntoView({ behavior: 'smooth', block: 'center' })
  el.classList.remove('block-flash')
  void el.offsetWidth // restart the animation
  el.classList.add('block-flash')
  setTimeout(() => el.classList.remove('block-flash'), 2200)
  return true
}
