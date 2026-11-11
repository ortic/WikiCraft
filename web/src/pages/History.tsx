import { t as tr } from '../i18n'
import { useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, RotateCcw } from 'lucide-react'
import { api, formatDate, type Page, type PmNode, type Version } from '../api'
import { Avatar, Spinner, useToast } from '../ui'

/** One line per text block (paragraph, heading, list item, cell, code block), in document order. */
function plain(n: PmNode): string[] {
  const out: string[] = []
  const inline = (x: PmNode, buf: string[]) => {
    if (x.text) buf.push(x.text)
    if (x.type === 'mention') buf.push(`@${x.attrs?.label ?? ''}`)
    if (x.type === 'hardBreak') buf.push(' ')
    x.content?.forEach((c) => inline(c, buf))
  }
  const isTextBlock = (x: PmNode) => !!x.content?.length && x.content.every((c) => c.type === 'text' || c.type === 'hardBreak' || c.type === 'mention' || c.type === 'image' || c.type === 'emoticon' || c.type === 'status' || c.type === 'date')
  const block = (x: PmNode, prefix: string) => {
    if (x.type === 'codeBlock' || isTextBlock(x)) {
      const buf: string[] = []
      inline(x, buf)
      const text = buf.join('')
      if (text.trim()) out.push(prefix + text)
      return
    }
    const p = x.type === 'listItem' ? '• ' : x.type === 'taskItem' ? (x.attrs?.checked ? '[x] ' : '[ ] ') : ''
    x.content?.forEach((c, i) => block(c, i === 0 ? prefix + p : ''))
  }
  for (const b of n.content ?? []) block(b, '')
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

type Op = '=' | '-' | '+'
type Word = { op: Op; text: string }
/** A line of the comparison: unchanged, removed, added, or changed (with its words marked). */
type Row = { op: Op; text: string } | { op: '~'; words: Word[] }

/** Word-level diff of one changed paragraph (spaces kept as their own tokens). */
function wordDiff(a: string, b: string): Word[] {
  const ta = a.split(/(\s+)/).filter((t) => t !== '')
  const tb = b.split(/(\s+)/).filter((t) => t !== '')
  return diff(ta, tb).map(({ op, text }) => ({ op, text }))
}

/** Pairs removed and added lines that follow each other into changed lines (word diff). */
function rows(lines: { op: Op; text: string }[]): Row[] {
  const out: Row[] = []
  for (let i = 0; i < lines.length; ) {
    if (lines[i].op !== '-') {
      out.push(lines[i++])
      continue
    }
    const removed: string[] = []
    while (i < lines.length && lines[i].op === '-') removed.push(lines[i++].text)
    const added: string[] = []
    while (i < lines.length && lines[i].op === '+') added.push(lines[i++].text)
    const pairs = Math.min(removed.length, added.length)
    for (let k = 0; k < pairs; k++) {
      // only similar lines count as "changed"; otherwise removed + added
      const words = wordDiff(removed[k], added[k])
      const same = words.filter((w) => w.op === '=' && w.text.trim()).length
      const total = Math.max(removed[k].split(/\s+/).length, added[k].split(/\s+/).length)
      if (same * 2 >= total) out.push({ op: '~', words })
      else out.push({ op: '-', text: removed[k] }, { op: '+', text: added[k] })
    }
    removed.slice(pairs).forEach((t) => out.push({ op: '-', text: t }))
    added.slice(pairs).forEach((t) => out.push({ op: '+', text: t }))
  }
  return out
}

function Words({ words, side }: { words: Word[]; side?: 'old' | 'new' }) {
  return (
    <>
      {words.map((w, i) =>
        w.op === '=' ? (
          <span key={i}>{w.text}</span>
        ) : w.op === '-' ? (
          side === 'new' ? null : <del key={i} className="diff-old">{w.text}</del>
        ) : side === 'old' ? null : (
          <ins key={i} className="diff-new">{w.text}</ins>
        ),
      )}
    </>
  )
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
  // the version to compare with (default: the one before, or "since your last visit")
  const [base, setBase] = useState<number | null>(null)
  const [view, setView] = useState<'inline' | 'split'>('inline')
  const [unchanged, setUnchanged] = useState(true)
  const sel = selected ?? versions.data?.[0]?.version ?? null
  const idx = versions.data?.findIndex((v) => v.version === sel) ?? -1
  const sinceMode = since != null && selected == null && base == null && sel != null && since < sel
  const prevVersion = base ?? (sinceMode ? since : idx >= 0 ? versions.data?.[idx + 1]?.version : undefined)
  const cur = useQuery({ queryKey: ['version', id, sel], queryFn: () => api.get<{ content: PmNode; title: string }>(`/pages/${id}/versions/${sel}`), enabled: sel != null })
  const prev = useQuery({ queryKey: ['version', id, prevVersion], queryFn: () => api.get<{ content: PmNode; title: string }>(`/pages/${id}/versions/${prevVersion}`), enabled: prevVersion != null })
  const lines = cur.data ? rows(diff(prev.data ? plain(prev.data.content) : [], plain(cur.data.content))) : []
  const changes = lines.filter((l) => l.op !== '=').length
  const shown = unchanged ? lines : lines.filter((l) => l.op !== '=')

  const restore = async () => {
    if (sel == null || !confirm(tr('Restore version {sel}? The current content becomes a new version.', { sel }))) return
    await api.post(`/pages/${id}/versions/${sel}/restore`)
    qc.invalidateQueries({ queryKey: ['page', id] })
    qc.invalidateQueries({ queryKey: ['versions', id] })
    toast(tr('Version {sel} restored', { sel }))
    nav(`/pages/${id}`)
  }
  return (
    <div className="content-pad">
      <Link to={`/pages/${id}`} className="row muted" style={{ marginBottom: 12 }}>
        <ArrowLeft size={15} />{' '}{tr('Back to page')}</Link>
      <h1 style={{ marginTop: 0 }}>{tr('History · {title}', { title: page.data?.title })}</h1>
      <div className="history-grid">
        <div className="card" style={{ padding: 6, maxHeight: '75vh', overflowY: 'auto' }}>
          {versions.isLoading && <Spinner />}
          {versions.data?.map((v) => (
            <div key={v.version} className={`menu-item ${v.version === sel ? 'selected' : ''}`} onClick={() => { setSelected(v.version); setBase(null) }}>
              <Avatar name={v.createdByName ?? '?'} id={v.createdBy ?? undefined} size={24} />
              <div style={{ minWidth: 0 }}>
                <div>
                  <b>{tr('v{version}', { version: v.version })}</b> <span className="faint small">{formatDate(v.createdAt)}</span>
                  {v.minor && <span className="badge" title={tr('Watchers weren\'t notified about this version')}>{tr('minor')}</span>}
                </div>
                <div className="mi-desc">{v.createdByName ?? tr('Unknown')}{v.message ? ` — ${v.message}` : ''}</div>
              </div>
            </div>
          ))}
        </div>
        <div className="card">
          <div className="row" style={{ marginBottom: 12, flexWrap: 'wrap', gap: 8 }}>
            <b>{tr('Version {sel}', { sel })}</b>
            <label className="row small" style={{ gap: 6 }}>{tr('compared with')}<select className="select" style={{ width: 'auto' }} aria-label={tr('Compare with version')} value={prevVersion ?? ''} onChange={(e) => setBase(e.target.value ? Number(e.target.value) : null)}>
                {!prevVersion && <option value="">{tr('nothing (first version)')}</option>}
                {versions.data?.filter((v) => v.version !== sel).map((v) => (
                  <option key={v.version} value={v.version}>{tr('v{version} · {date}', { version: v.version, date: formatDate(v.createdAt) })}{sinceMode && v.version === since ? ' ' + tr('(your last visit)') + '' : ''}
                  </option>
                ))}
              </select>
            </label>
            <span className="faint small">{tr('{count} changes', { count: changes })}</span>
            <span className="spacer" />
            <div className="seg" role="group" aria-label={tr('Show the comparison')}>
              <button className={`btn btn-sm${view === 'inline' ? ' active' : ''}`} aria-pressed={view === 'inline'} onClick={() => setView('inline')}>{tr('Inline')}</button>
              <button className={`btn btn-sm${view === 'split' ? ' active' : ''}`} aria-pressed={view === 'split'} onClick={() => setView('split')}>{tr('Side by side')}</button>
            </div>
            <label className="row small"><input type="checkbox" checked={unchanged} onChange={(e) => setUnchanged(e.target.checked)} />{' '}{tr('Show unchanged')}</label>
            {page.data?.access.edit && sel !== versions.data?.[0]?.version && (
              <button className="btn" onClick={restore}>
                <RotateCcw size={14} />{' '}{tr('Restore this version')}</button>
            )}
          </div>
          {cur.isLoading && <Spinner />}
          {cur.data && cur.data.title !== prev.data?.title && prev.data && (
            <p className="small">{tr('Title:')}{' '}<span className="diff-old">{prev.data.title}</span> → <span className="diff-new">{cur.data.title}</span>
            </p>
          )}
          {view === 'inline' ? (
            <div className="diff-lines">
              {shown.map((l, i) =>
                l.op === '~' ? (
                  <div key={i} className="diff-line changed">
                    <span className="diff-mark" aria-label={tr('changed')}>~</span>
                    <Words words={l.words} />
                  </div>
                ) : (
                  <div key={i} className={`diff-line${l.op === '-' ? ' removed diff-old' : l.op === '+' ? ' added diff-new' : ''}`}>
                    <span className="diff-mark" aria-label={l.op === '-' ? tr('removed') : l.op === '+' ? tr('added') : undefined}>{l.op === '=' ? ' ' : l.op}</span>
                    {l.text || ' '}
                  </div>
                ),
              )}
            </div>
          ) : (
            <div className="diff-split">
              <div className="diff-split-head faint small">{tr('v{value}', { value: prevVersion ?? '–' })}</div>
              <div className="diff-split-head faint small">{tr('v{sel}', { sel })}</div>
              {shown.map((l, i) =>
                l.op === '~' ? (
                  [
                    <div key={`${i}a`} className="diff-cell changed"><Words words={l.words} side="old" /></div>,
                    <div key={`${i}b`} className="diff-cell changed"><Words words={l.words} side="new" /></div>,
                  ]
                ) : (
                  [
                    <div key={`${i}a`} className={`diff-cell${l.op === '-' ? ' removed diff-old' : ''}`}>{l.op === '+' ? '' : l.text}</div>,
                    <div key={`${i}b`} className={`diff-cell${l.op === '+' ? ' added diff-new' : ''}`}>{l.op === '-' ? '' : l.text}</div>,
                  ]
                ),
              )}
            </div>
          )}
          {!unchanged && lines.length > shown.length && <p className="faint small">{tr('{value} unchanged lines hidden.', { value: lines.length - shown.length })}</p>}
        </div>
      </div>
    </div>
  )
}
