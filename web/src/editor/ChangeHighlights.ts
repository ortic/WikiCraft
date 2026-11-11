// Tints the blocks that changed since the reader's last visit (block ids from the server's
// `changedSince.blocks`). Set them with `setChangeHighlights(editor, ids)`; an empty list clears.

import { t } from '../i18n'
import { Extension, type Editor } from '@tiptap/core'
import { Plugin, PluginKey } from '@tiptap/pm/state'
import { Decoration, DecorationSet } from '@tiptap/pm/view'
import type { Node as PMNode } from '@tiptap/pm/model'
import { BLOCK_ID_ATTR } from './BlockIds'

interface HighlightState {
  ids: Set<string>
  set: DecorationSet
}

const key = new PluginKey<HighlightState>('changeHighlights')

function decorate(doc: PMNode, ids: Set<string>): DecorationSet {
  if (!ids.size) return DecorationSet.empty
  const decos: Decoration[] = []
  doc.descendants((node, pos) => {
    const id = node.attrs?.[BLOCK_ID_ATTR] as string | undefined
    if (id && ids.has(id)) {
      decos.push(Decoration.node(pos, pos + node.nodeSize, { class: 'changed-block', title: t('Changed since your last visit') }))
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
      new Plugin<HighlightState>({
        key,
        state: {
          init: () => ({ ids: new Set(), set: DecorationSet.empty }),
          apply(tr, prev) {
            const ids = tr.getMeta(key) as Set<string> | undefined
            if (ids) return { ids, set: decorate(tr.doc, ids) }
            // recomputed, not mapped: the collaboration sync may replace the whole document
            if (tr.docChanged && prev.ids.size) return { ids: prev.ids, set: decorate(tr.doc, prev.ids) }
            return prev
          },
        },
        props: {
          decorations: (state) => key.getState(state)?.set,
        },
      }),
    ]
  },
})

export function setChangeHighlights(editor: Editor, ids: string[]) {
  if (editor.isDestroyed) return
  editor.view.dispatch(editor.state.tr.setMeta(key, new Set(ids)).setMeta('addToHistory', false))
}
