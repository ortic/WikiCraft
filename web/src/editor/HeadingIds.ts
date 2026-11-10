// Gives every heading an `id` (decoration only, not stored), the same slug the server uses for
// exports and shared pages, so `#anchor` links work inside the app too.

import { Extension } from '@tiptap/core'
import { Plugin, PluginKey } from '@tiptap/pm/state'
import { Decoration, DecorationSet } from '@tiptap/pm/view'
import type { Node as PMNode } from '@tiptap/pm/model'
import { slugify } from './nodes'

/** Heading ids in document order: slug, then slug-1, slug-2 … for repeats (as the server does). */
export function headingIds(doc: PMNode): { pos: number; size: number; id: string; text: string; level: number }[] {
  const used = new Map<string, number>()
  const out: { pos: number; size: number; id: string; text: string; level: number }[] = []
  doc.descendants((node, pos) => {
    if (node.type.name !== 'heading') return true
    const text = node.textContent
    const base = slugify(text)
    const n = used.get(base) ?? 0
    used.set(base, n + 1)
    out.push({ pos, size: node.nodeSize, id: n === 0 ? base : `${base}-${n}`, text, level: Number(node.attrs.level ?? 1) })
    return false
  })
  return out
}

const key = new PluginKey<DecorationSet>('headingIds')

export const HeadingIds = Extension.create({
  name: 'headingIds',
  addProseMirrorPlugins() {
    const build = (doc: PMNode) => DecorationSet.create(doc, headingIds(doc).map((h) => Decoration.node(h.pos, h.pos + h.size, { id: h.id })))
    return [
      new Plugin<DecorationSet>({
        key,
        state: {
          init: (_, state) => build(state.doc),
          apply: (tr, old) => (tr.docChanged ? build(tr.doc) : old),
        },
        props: { decorations: (state) => key.getState(state) },
      }),
    ]
  },
})
