// Indentation of paragraphs and headings (`indent` attribute, 30px per level, like
// Confluence). Tab at the start of a paragraph and Ctrl+] indent, Shift+Tab and Ctrl+[ outdent;
// Tab elsewhere still leaves the editor, so keyboard users are never trapped. Lists and tables
// keep their own Tab handling.

import { Extension } from '@tiptap/core'
import type { ResolvedPos } from '@tiptap/pm/model'

export const MAX_INDENT = 8
const TYPES = ['paragraph', 'heading']
/** blocks whose paragraphs aren't indented (lists nest, tables move between cells) */
const CONTAINERS = ['listItem', 'taskItem', 'tableCell', 'tableHeader']

declare module '@tiptap/core' {
  interface Commands<ReturnType> {
    indent: {
      indent: () => ReturnType
      outdent: () => ReturnType
    }
  }
}

function inContainer($pos: ResolvedPos) {
  for (let d = $pos.depth; d > 0; d--) if (CONTAINERS.includes($pos.node(d).type.name)) return true
  return false
}

/** Levels from a pasted `margin-left` (px, pt or em). */
function levelsFromMargin(margin: string): number {
  const m = /^(-?[\d.]+)\s*(px|pt|em|rem)?$/.exec(margin.trim())
  if (!m) return 0
  const n = Number(m[1])
  const px = m[2] === 'pt' ? (n * 4) / 3 : m[2] === 'em' || m[2] === 'rem' ? n * 16 : n
  return Math.max(0, Math.min(MAX_INDENT, Math.round(px / 30)))
}

export const Indent = Extension.create({
  name: 'indent',
  // after lists and tables, which use Tab themselves
  priority: 50,

  addGlobalAttributes() {
    return [
      {
        types: TYPES,
        attributes: {
          indent: {
            default: null,
            parseHTML: (el: HTMLElement) => {
              const n = Number(el.getAttribute('data-indent')) || levelsFromMargin(el.style.marginLeft || '')
              return n > 0 ? Math.min(MAX_INDENT, Math.round(n)) : null
            },
            renderHTML: (attrs: { indent?: number | null }) => (attrs.indent ? { 'data-indent': String(attrs.indent) } : {}),
          },
        },
      },
    ]
  },

  addCommands() {
    const change =
      (delta: number) =>
      () =>
      ({ tr, state, dispatch }: { tr: import('@tiptap/pm/state').Transaction; state: import('@tiptap/pm/state').EditorState; dispatch?: unknown }) => {
        const { from, to } = state.selection
        let changed = false
        state.doc.nodesBetween(from, to, (node, pos) => {
          if (CONTAINERS.includes(node.type.name) || node.type.name === 'codeBlock') return false
          if (!TYPES.includes(node.type.name)) return true
          const cur = Number(node.attrs.indent) || 0
          const next = Math.max(0, Math.min(MAX_INDENT, cur + delta))
          if (next !== cur) {
            if (dispatch) tr.setNodeMarkup(pos, undefined, { ...node.attrs, indent: next || null })
            changed = true
          }
          return false
        })
        return changed
      }
    return { indent: change(1), outdent: change(-1) }
  },

  addKeyboardShortcuts() {
    const here = () => {
      const { $from } = this.editor.state.selection
      return TYPES.includes($from.parent.type.name) && !inContainer($from) ? $from : null
    }
    /** Where the caret is: the browser's selection can be ahead of the editor's right after a
     * key like Home (the editor reads it on the next selectionchange). */
    const caret = () => {
      const { view, state } = this.editor
      const dom = (view.root as Document).getSelection?.()
      if (dom && dom.rangeCount && dom.anchorNode && view.dom.contains(dom.anchorNode)) {
        if (!dom.isCollapsed) return null
        try {
          return state.doc.resolve(view.posAtDOM(dom.anchorNode, dom.anchorOffset))
        } catch {
          // fall back to the editor's selection
        }
      }
      return state.selection.empty ? state.selection.$from : null
    }
    return {
      Tab: () => {
        const $at = caret()
        if (!$at || $at.parentOffset !== 0 || !TYPES.includes($at.parent.type.name) || inContainer($at)) return false
        return this.editor.chain().setTextSelection($at.pos).indent().run()
      },
      'Shift-Tab': () => {
        const $from = here()
        return !!$from && (Number($from.parent.attrs.indent) || 0) > 0 && this.editor.commands.outdent()
      },
      'Mod-]': () => this.editor.commands.indent(),
      'Mod-[': () => this.editor.commands.outdent(),
    }
  },
})
