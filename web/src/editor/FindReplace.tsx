// Find & replace in the page: a ProseMirror plugin that highlights every match (also across
// formatting boundaries within a paragraph) and a floating bar (Ctrl+F, Ctrl+H to replace).
// Replacing goes through normal transactions, so it is collaborative and undoable.

import { t } from '../i18n'
import { useEffect, useReducer, useRef, useState } from 'react'
import { Extension, type Editor } from '@tiptap/core'
import { Plugin, PluginKey, TextSelection } from '@tiptap/pm/state'
import { Decoration, DecorationSet } from '@tiptap/pm/view'
import type { Node as PMNode } from '@tiptap/pm/model'
import { ChevronDown, ChevronUp, Replace, X } from 'lucide-react'

interface Match {
  from: number
  to: number
}

interface FindState {
  query: string
  caseSensitive: boolean
  current: number
  matches: Match[]
  deco: DecorationSet
}

export const findKey = new PluginKey<FindState>('findReplace')

/** Matches of `query` in text, joined across adjacent text nodes (marks) of each block. */
export function findMatches(doc: PMNode, query: string, caseSensitive: boolean): Match[] {
  const out: Match[] = []
  if (!query) return out
  const q = caseSensitive ? query : query.toLowerCase()
  doc.descendants((node, pos) => {
    if (!node.isTextblock) return true
    let run = ''
    let start = -1
    const flush = () => {
      if (run) {
        const hay = caseSensitive ? run : run.toLowerCase()
        // lower-casing can change the length of a few characters; skip those blocks' offsets
        if (hay.length === run.length) {
          let i = hay.indexOf(q)
          while (i !== -1 && out.length < 5000) {
            out.push({ from: start + i, to: start + i + q.length })
            i = hay.indexOf(q, i + q.length)
          }
        }
      }
      run = ''
      start = -1
    }
    node.forEach((child, offset) => {
      if (child.isText) {
        if (start === -1) start = pos + 1 + offset
        run += child.text ?? ''
      } else {
        flush()
      }
    })
    flush()
    return false
  })
  return out
}

const empty: FindState = { query: '', caseSensitive: false, current: 0, matches: [], deco: DecorationSet.empty }

export const FindReplace = Extension.create({
  name: 'findReplace',
  addProseMirrorPlugins() {
    return [
      new Plugin<FindState>({
        key: findKey,
        state: {
          init: () => empty,
          apply(tr, prev, _old, state) {
            const meta = tr.getMeta(findKey) as Partial<FindState> | undefined
            if (!meta && (!tr.docChanged || !prev.query)) return prev
            const query = meta?.query ?? prev.query
            const caseSensitive = meta?.caseSensitive ?? prev.caseSensitive
            if (!query) return empty
            const matches = findMatches(state.doc, query, caseSensitive)
            let current = meta?.current ?? prev.current
            if (current >= matches.length) current = 0
            const deco = DecorationSet.create(
              state.doc,
              matches.map((m, i) => Decoration.inline(m.from, m.to, { class: i === current ? 'find-match find-current' : 'find-match' })),
            )
            return { query, caseSensitive, current, matches, deco }
          },
        },
        props: {
          decorations: (state) => findKey.getState(state)?.deco,
        },
      }),
    ]
  },
})

export function FindBar({ editor, withReplace, initial, onClose }: { editor: Editor; withReplace: boolean; initial: string; onClose: () => void }) {
  const [query, setQuery] = useState(initial)
  const [replacement, setReplacement] = useState('')
  const [caseSensitive, setCaseSensitive] = useState(false)
  const [, rerender] = useReducer((x: number) => x + 1, 0)
  const input = useRef<HTMLInputElement>(null)

  useEffect(() => {
    editor.on('transaction', rerender)
    return () => {
      editor.off('transaction', rerender)
    }
  }, [editor])
  useEffect(() => {
    input.current?.focus()
    input.current?.select()
  }, [withReplace])
  useEffect(() => {
    editor.view.dispatch(editor.state.tr.setMeta(findKey, { query, caseSensitive, current: 0 }))
  }, [editor, query, caseSensitive])
  // clear the highlights when the bar closes
  useEffect(() => () => {
    if (!editor.isDestroyed) editor.view.dispatch(editor.state.tr.setMeta(findKey, { query: '' }))
  }, [editor])

  // Escape closes the bar even when focus left it (e.g. a button that just got disabled)
  const closeRef = useRef(onClose)
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || document.querySelector('.modal-backdrop, .lightbox')) return
      const active = document.activeElement
      if (active && active !== document.body && !active.closest('.find-bar')) return
      e.preventDefault()
      closeRef.current()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const st = findKey.getState(editor.state) ?? empty
  const total = st.matches.length
  const editable = editor.isEditable

  const go = (dir: 1 | -1) => {
    if (!total) return
    const current = (st.current + dir + total) % total
    const m = st.matches[current]
    editor.view.dispatch(editor.state.tr.setMeta(findKey, { current }).setSelection(TextSelection.create(editor.state.doc, m.from, m.to)).scrollIntoView())
  }
  const replaceOne = () => {
    const m = st.matches[st.current]
    if (!m || !editable) return
    editor.view.dispatch(editor.state.tr.insertText(replacement, m.from, m.to))
  }
  const replaceAll = () => {
    if (!total || !editable) return
    const tr = editor.state.tr
    for (const m of [...st.matches].reverse()) tr.insertText(replacement, m.from, m.to)
    editor.view.dispatch(tr)
  }
  const close = () => {
    onClose()
    if (editable) editor.commands.focus()
  }
  closeRef.current = close
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter' && (e.target as HTMLElement).dataset.role === 'find') {
      e.preventDefault()
      go(e.shiftKey ? -1 : 1)
    } else if (e.key === 'Enter' && (e.target as HTMLElement).dataset.role === 'replace') {
      e.preventDefault()
      if (e.ctrlKey || e.metaKey) replaceAll()
      else replaceOne()
    }
  }

  return (
    <div className="find-bar" role="search" onKeyDown={onKey}>
      <div className="row" style={{ gap: 6 }}>
        <input ref={input} data-role="find" className="input" placeholder={t('Find in page')} value={query} onChange={(e) => setQuery(e.target.value)} aria-label={t('Find')} />
        <span className="faint small find-count">{query ? (total ? t('{value} of {total}', { value: st.current + 1, total }) : t('No results')) : ''}</span>
        <button className={`icon-btn ${caseSensitive ? 'active' : ''}`} title={t('Match case')} aria-pressed={caseSensitive} onClick={() => setCaseSensitive(!caseSensitive)}>
          <span style={{ fontSize: 12, fontWeight: 700 }}>{t('Aa')}</span>
        </button>
        <button className="icon-btn" title={t('Previous (Shift+Enter)')} disabled={!total} onClick={() => go(-1)}><ChevronUp size={16} /></button>
        <button className="icon-btn" title={t('Next (Enter)')} disabled={!total} onClick={() => go(1)}><ChevronDown size={16} /></button>
        <button className="icon-btn" title={t('Close (Esc)')} onClick={close}><X size={16} /></button>
      </div>
      {withReplace && editable && (
        <div className="row" style={{ gap: 6, marginTop: 6 }}>
          <input data-role="replace" className="input" placeholder={t('Replace with')} value={replacement} onChange={(e) => setReplacement(e.target.value)} aria-label={t('Replace with')} />
          <button className="btn btn-sm" disabled={!total} onClick={replaceOne}><Replace size={14} />{' '}{t('Replace')}</button>
          <button className="btn btn-sm" disabled={!total} onClick={replaceAll}>{t('All')}</button>
        </div>
      )}
    </div>
  )
}
