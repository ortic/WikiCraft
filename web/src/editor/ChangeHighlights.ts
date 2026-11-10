// Tints the blocks that changed since the reader's last visit (block ids from the server's
// `changedSince.blocks`). Set them with `setChangeHighlights(editor, ids)`; an empty list clears.

import { Extension, type Editor } from '@tiptap/core'
import { Plugin, PluginKey } from '@tiptap/pm/state'
import { Decoration, DecorationSet } from '@tiptap/pm/view'
import type { Node as PMNode } from '@tiptap/pm/model'
import { BLOCK_ID_ATTR } from './BlockIds'

const key = new PluginKey<DecorationSet>('changeHighlights')

function decorate(doc: PMNode, ids: Set<string>): DecorationSet {
  if (!ids.size) return DecorationSet.empty
  const decos: Decoration[] = []
  doc.descendants((node, pos) => {
    const id = node.attrs?.[BLOCK_ID_ATTR] as string | undefined
    if (id && ids.has(id)) {
      decos.push(Decoration.node(pos, pos + node.nodeSize, { class: 'changed-block', title: 'Changed since your last visit' }))
      return false
    }
    return true
  })
  return DecorationSet.create(doc, decos)
}

export const ChangeHighlights = Extension.create({
  name: 'changeHighlights',
  addProseMirrorPlugins() {
    return [
      new Plugin<DecorationSet>({
        key,
        state: {
          init: () => DecorationSet.empty,
          apply(tr, set) {
            const ids = tr.getMeta(key) as Set<string> | undefined
            if (ids) return decorate(tr.doc, ids)
            return tr.docChanged ? set.map(tr.mapping, tr.doc) : set
          },
        },
        props: {
          decorations: (state) => key.getState(state),
        },
      }),
    ]
  },
})

export function setChangeHighlights(editor: Editor, ids: string[]) {
  if (editor.isDestroyed) return
  editor.view.dispatch(editor.state.tr.setMeta(key, new Set(ids)).setMeta('addToHistory', false))
}
