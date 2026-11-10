import { useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, RotateCcw } from 'lucide-react'
import { api, formatDate, type Page, type PmNode, type Version } from '../api'
import { Avatar, Spinner, useToast } from '../ui'

function plain(n: PmNode): string[] {
  const out: string[] = []
  const walk = (x: PmNode, buf: string[]) => {
    if (x.text) buf.push(x.text)
    if (x.type === 'mention') buf.push(`@${x.attrs?.label ?? ''}`)
    x.content?.forEach((c) => walk(c, buf))
  }
  for (const b of n.content ?? []) {
    const buf: string[] = []
    walk(b, buf)
    out.push(buf.join(''))
  }
  return out
}

/** Paragraph-level diff (LCS) between two versions. */
function diff(a: string[], b: string[]): { op: '=' | '-' | '+'; text: string }[] {
  const n = a.length
  const m = b.length
  if (n * m > 4_000_000) return [...a.map((t) => ({ op: '-' as const, text: t })), ...b.map((t) => ({ op: '+' as const, text: t }))]
  const dp = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0))
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
  const out: { op: '=' | '-' | '+'; text: string }[] = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      out.push({ op: '=', text: a[i] })
      i++
      j++
    } else if (dp[i + 1][j] >= dp[i][j + 1]) out.push({ op: '-', text: a[i++] })
    else out.push({ op: '+', text: b[j++] })
  }
  while (i < n) out.push({ op: '-', text: a[i++] })
  while (j < m) out.push({ op: '+', text: b[j++] })
  return out
}

export default function History() {
  const { id } = useParams()
  const nav = useNavigate()
  const qc = useQueryClient()
  const toast = useToast()
  const page = useQuery({ queryKey: ['page', id], queryFn: () => api.get<Page>(`/pages/${id}`) })
  const versions = useQuery({ queryKey: ['versions', id], queryFn: () => api.get<Version[]>(`/pages/${id}/versions`) })
  const [params] = useSearchParams()
  // ?since=<version>: everything that changed after that version (from "changed since your last visit")
  const since = Number(params.get('since')) || null
  const [selected, setSelected] = useState<number | null>(null)
  const sel = selected ?? versions.data?.[0]?.version ?? null
  const idx = versions.data?.findIndex((v) => v.version === sel) ?? -1
  const sinceMode = since != null && selected == null && sel != null && since < sel
  const prevVersion = sinceMode ? since : idx >= 0 ? versions.data?.[idx + 1]?.version : undefined
  const cur = useQuery({ queryKey: ['version', id, sel], queryFn: () => api.get<{ content: PmNode; title: string }>(`/pages/${id}/versions/${sel}`), enabled: sel != null })
  const prev = useQuery({ queryKey: ['version', id, prevVersion], queryFn: () => api.get<{ content: PmNode; title: string }>(`/pages/${id}/versions/${prevVersion}`), enabled: prevVersion != null })
  const lines = cur.data ? diff(prev.data ? plain(prev.data.content) : [], plain(cur.data.content)) : []

  const restore = async () => {
    if (sel == null || !confirm(`Restore version ${sel}? The current content becomes a new version.`)) return
    await api.post(`/pages/${id}/versions/${sel}/restore`)
    qc.invalidateQueries({ queryKey: ['page', id] })
    qc.invalidateQueries({ queryKey: ['versions', id] })
    toast(`Version ${sel} restored`)
    nav(`/pages/${id}`)
  }
  return (
    <div className="content-pad">
      <Link to={`/pages/${id}`} className="row muted" style={{ marginBottom: 12 }}>
        <ArrowLeft size={15} /> Back to page
      </Link>
      <h1 style={{ marginTop: 0 }}>History · {page.data?.title}</h1>
      <div style={{ display: 'grid', gridTemplateColumns: '300px 1fr', gap: 20, alignItems: 'start' }}>
        <div className="card" style={{ padding: 6, maxHeight: '75vh', overflowY: 'auto' }}>
          {versions.isLoading && <Spinner />}
          {versions.data?.map((v) => (
            <div key={v.version} className={`menu-item ${v.version === sel ? 'selected' : ''}`} onClick={() => setSelected(v.version)}>
              <Avatar name={v.createdByName ?? '?'} size={24} />
              <div style={{ minWidth: 0 }}>
                <div>
                  <b>v{v.version}</b> <span className="faint small">{formatDate(v.createdAt)}</span>
                </div>
                <div className="mi-desc">{v.createdByName ?? 'Unknown'}{v.message ? ` — ${v.message}` : ''}</div>
              </div>
            </div>
          ))}
        </div>
        <div className="card">
          <div className="row" style={{ marginBottom: 12 }}>
            <b>Version {sel}</b>
            <span className="faint small">{prevVersion ? `changes since v${prevVersion}${sinceMode ? ' (your last visit)' : ''}` : 'first version'}</span>
            <span className="spacer" />
            {page.data?.access.edit && sel !== versions.data?.[0]?.version && (
              <button className="btn" onClick={restore}>
                <RotateCcw size={14} /> Restore this version
              </button>
            )}
          </div>
          {cur.isLoading && <Spinner />}
          {cur.data && cur.data.title !== prev.data?.title && prev.data && (
            <p className="small">
              Title: <span className="diff-old">{prev.data.title}</span> → <span className="diff-new">{cur.data.title}</span>
            </p>
          )}
          <div style={{ fontSize: 14 }}>
            {lines.map((l, i) => (
              <div key={i} className={l.op === '-' ? 'diff-old' : l.op === '+' ? 'diff-new' : ''} style={{ padding: '2px 8px', whiteSpace: 'pre-wrap', textDecoration: l.op === '-' ? 'line-through' : undefined }}>
                {l.op === '=' ? ' ' : l.op} {l.text || ' '}
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  )
}
