import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { FileText, Clock } from 'lucide-react'
import { api, type PageListItem, type SearchHit } from '../api'

export default function SearchPalette({ onClose }: { onClose: () => void }) {
  const [q, setQ] = useState('')
  const [debounced, setDebounced] = useState('')
  const [sel, setSel] = useState(0)
  const nav = useNavigate()
  useEffect(() => {
    const t = setTimeout(() => setDebounced(q.trim()), 150)
    return () => clearTimeout(t)
  }, [q])
  const results = useQuery({ queryKey: ['search', debounced], queryFn: () => api.get<SearchHit[]>(`/search?q=${encodeURIComponent(debounced)}&limit=12`), enabled: debounced.length > 0 })
  const recent = useQuery({ queryKey: ['recent'], queryFn: () => api.get<{ viewed: PageListItem[] }>('/recent'), enabled: debounced.length === 0, retry: false })
  const items: { id: string; title: string; sub: string; snippet?: string }[] = debounced
    ? (results.data ?? []).map((h) => ({ id: h.pageId, title: h.title, sub: h.spaceName, snippet: h.snippet }))
    : (recent.data?.viewed ?? []).slice(0, 8).map((p) => ({ id: p.id, title: p.title, sub: p.spaceName }))
  useEffect(() => setSel(0), [debounced])
  const go = (id: string) => {
    onClose()
    nav(`/pages/${id}`)
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
      if (items[sel]) go(items[sel].id)
      else if (q.trim()) {
        onClose()
        nav(`/search?q=${encodeURIComponent(q.trim())}`)
      }
    }
  }
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal palette">
        <input autoFocus placeholder="Search pages, or jump to a recent one…" value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={onKey} />
        <div className="palette-results">
          {!debounced && <div className="menu-group">Recently viewed</div>}
          {items.map((it, i) => (
            <a key={it.id} className={`result ${i === sel ? 'selected' : ''}`} onMouseEnter={() => setSel(i)} onClick={() => go(it.id)}>
              <div className="row">
                {debounced ? <FileText size={14} className="faint" /> : <Clock size={14} className="faint" />}
                <span className="r-title">{it.title}</span>
                <span className="faint small">· {it.sub}</span>
              </div>
              {/* snippet HTML is produced by the server's search engine and contains only escaped text + <b> */}
              {it.snippet && <div className="r-snippet" dangerouslySetInnerHTML={{ __html: it.snippet }} />}
            </a>
          ))}
          {debounced && results.isFetched && items.length === 0 && <div className="empty">No pages match “{debounced}”.</div>}
          {debounced && (
            <a className="result faint small" onClick={() => { onClose(); nav(`/search?q=${encodeURIComponent(debounced)}`) }}>
              See all results for “{debounced}” →
            </a>
          )}
        </div>
      </div>
    </div>
  )
}
