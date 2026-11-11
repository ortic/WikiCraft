import { t as tr } from '../i18n'
import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { FileText, Clock, CornerDownLeft } from 'lucide-react'
import type { ReactNode } from 'react'
import { api, type PageListItem, type SearchHit } from '../api'

/** Something the palette can do besides opening a page ("Create page", "Go to My tasks", …). */
export interface PaletteAction {
  id: string
  label: string
  /** extra words it matches */
  keywords?: string
  hint?: string
  icon: ReactNode
  run: () => void
}

type Item = { kind: 'page'; id: string; title: string; sub: string; snippet?: string } | { kind: 'action'; action: PaletteAction }

function matches(a: PaletteAction, q: string): boolean {
  const hay = `${a.label} ${a.keywords ?? ''} ${a.hint ?? ''}`.toLowerCase()
  return q
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => hay.includes(w))
}

/** Ctrl+K: jump to pages and run actions. Typing matches both; a leading `>` shows actions only. */
export default function SearchPalette({ onClose, actions = [] }: { onClose: () => void; actions?: PaletteAction[] }) {
  const [q, setQ] = useState('')
  const [debounced, setDebounced] = useState('')
  const [sel, setSel] = useState(0)
  const nav = useNavigate()
  useEffect(() => {
    const t = setTimeout(() => setDebounced(q.trim()), 150)
    return () => clearTimeout(t)
  }, [q])
  const actionsOnly = q.trimStart().startsWith('>')
  const term = actionsOnly ? q.trimStart().slice(1).trim() : debounced
  const results = useQuery({ queryKey: ['search', debounced], queryFn: () => api.get<SearchHit[]>(`/search?q=${encodeURIComponent(debounced)}&limit=12`), enabled: debounced.length > 0 && !actionsOnly })
  const recent = useQuery({ queryKey: ['recent'], queryFn: () => api.get<{ viewed: PageListItem[] }>('/recent'), enabled: debounced.length === 0, retry: false })
  const matchingActions = actionsOnly ? actions.filter((a) => !term || matches(a, term)) : term ? actions.filter((a) => matches(a, term)).slice(0, 4) : []
  const pages: Item[] = actionsOnly
    ? []
    : debounced
      ? (results.data ?? []).map((h) => ({ kind: 'page' as const, id: h.pageId, title: h.title, sub: h.spaceName, snippet: h.snippet }))
      : (recent.data?.viewed ?? []).slice(0, 8).map((p) => ({ kind: 'page' as const, id: p.id, title: p.title, sub: p.spaceName }))
  const items: Item[] = [...matchingActions.map((a) => ({ kind: 'action' as const, action: a })), ...pages]
  useEffect(() => setSel(0), [debounced, actionsOnly])
  const go = (it: Item) => {
    onClose()
    if (it.kind === 'page') nav(`/pages/${it.id}`)
    else it.action.run()
  }
  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') onClose()
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setSel((s) => Math.min(s + 1, items.length - 1))
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      setSel((s) => Math.max(s - 1, 0))
    }
    if (e.key === 'Enter') {
      if (items[sel]) go(items[sel])
      else if (q.trim() && !actionsOnly) {
        onClose()
        nav(`/search?q=${encodeURIComponent(q.trim())}`)
      }
    }
  }
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal palette">
        <input
          autoFocus
          placeholder={tr('Search pages, or type > for actions…')}
          aria-label={tr('Search pages and actions')}
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={onKey}
        />
        <div className="palette-results">
          {items.map((it, i) => {
            const header =
              i === 0 && it.kind === 'action' ? tr('Actions') : it.kind === 'page' && (i === 0 || items[i - 1].kind === 'action') ? (debounced ? tr('Pages') : tr('Recently viewed')) : null
            return (
              <div key={it.kind === 'page' ? it.id : `a-${it.action.id}`}>
                {header && <div className="menu-group">{header}</div>}
                {it.kind === 'action' ? (
                  <a className={`result ${i === sel ? 'selected' : ''}`} onMouseEnter={() => setSel(i)} onClick={() => go(it)}>
                    <div className="row">
                      <span className="faint palette-action-icon">{it.action.icon}</span>
                      <span className="r-title">{it.action.label}</span>
                      {it.action.hint && <span className="faint small">· {it.action.hint}</span>}
                      <span className="spacer" />
                      {i === sel && <CornerDownLeft size={13} className="faint" />}
                    </div>
                  </a>
                ) : (
                  <a className={`result ${i === sel ? 'selected' : ''}`} onMouseEnter={() => setSel(i)} onClick={() => go(it)}>
                    <div className="row">
                      {debounced ? <FileText size={14} className="faint" /> : <Clock size={14} className="faint" />}
                      <span className="r-title">{it.title}</span>
                      <span className="faint small">· {it.sub}</span>
                    </div>
                    {/* snippet HTML is produced by the server's search engine and contains only escaped text + <b> */}
                    {it.snippet && <div className="r-snippet" dangerouslySetInnerHTML={{ __html: it.snippet }} />}
                  </a>
                )}
              </div>
            )
          })}
          {actionsOnly && items.length === 0 && <div className="empty">{tr('No action matches “{term}”.', { term })}</div>}
          {debounced && !actionsOnly && results.isFetched && pages.length === 0 && <div className="empty">{tr('No pages match “{debounced}”.', { debounced })}</div>}
          {debounced && !actionsOnly && (
            <a className="result faint small" onClick={() => { onClose(); nav(`/search?q=${encodeURIComponent(debounced)}`) }}>{tr('See all results for “{debounced}” →', { debounced })}</a>
          )}
        </div>
      </div>
    </div>
  )
}
