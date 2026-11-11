import { useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { BookmarkPlus, Braces, FileText, Sparkles, X } from 'lucide-react'
import { api, formatDate, type SearchHit, type Space } from '../api'
import { Spinner, useMe } from '../ui'

interface Answer {
  answer: string
  answerHtml: string
  sources: { n: number; pageId: string; title: string; spaceName: string }[]
  model: string | null
}

/** "Ask the wiki": an answer written from the pages the reader can see, with its sources. */
function AnswerCard({ state }: { state: { loading: boolean; error?: string; data?: Answer; question: string } }) {
  return (
    <div className="card ai-answer">
      <div className="row ai-answer-head">
        <Sparkles size={16} /> <b>Answer</b>
        <span className="faint small" style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{state.question}</span>
      </div>
      {state.loading && (
        <div className="row faint"><Spinner /> Reading the wiki…</div>
      )}
      {state.error && <p className="error-text">{state.error}</p>}
      {state.data && (
        <>
          {/* server-rendered from Markdown: escaped text, safe links only */}
          <div className="ai-answer-body prose" dangerouslySetInnerHTML={{ __html: state.data.answerHtml }} />
          {state.data.sources.length > 0 && (
            <div className="ai-sources">
              {state.data.sources.map((s) => (
                <Link key={s.pageId} to={`/pages/${s.pageId}`} className="ai-source" title={s.spaceName}>
                  <span className="ai-source-n">{s.n}</span> <FileText size={13} /> {s.title}
                </Link>
              ))}
            </div>
          )}
          <div className="faint small" style={{ marginTop: 8 }}>
            Written by AI{state.data.model ? ` (${state.data.model})` : ''} from pages you can access. Check the sources before relying on it.
          </div>
        </>
      )}
    </div>
  )
}

interface CqlPage {
  id: string
  title: string
  kind: string
  spaceKey: string
  spaceName: string
  updatedAt: string
  updatedByName: string | null
  labels: string[]
  excerpt: string
}

const CQL_EXAMPLE = 'space = ENG and label = runbook and lastModified > now("-30d") order by title'

/** Search with a query (Confluence's CQL): fields, operators and functions, results in its order. */
function CqlSearch({ initial, onExit, onRun }: { initial: string; onExit: () => void; onRun: (cql: string) => void }) {
  const [input, setInput] = useState(initial)
  const results = useQuery({
    queryKey: ['cql', initial],
    queryFn: () => api.get<CqlPage[]>(`/cql?cql=${encodeURIComponent(initial)}&limit=200`),
    enabled: !!initial.trim(),
    retry: false,
  })
  const error = results.error instanceof Error ? results.error.message : null
  return (
    <div className="content-pad">
      <form className="cql-form" onSubmit={(e) => { e.preventDefault(); onRun(input.trim()) }}>
        <label className="field" style={{ marginBottom: 6 }}>
          <span className="row" style={{ gap: 6 }}><Braces size={15} /> Query</span>
          <textarea
            className="input cql-input"
            rows={3}
            value={input}
            spellCheck={false}
            placeholder={CQL_EXAMPLE}
            autoFocus
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); onRun(input.trim()) } }}
          />
        </label>
        <div className="row" style={{ gap: 8 }}>
          <button className="btn btn-primary">Run query</button>
          <button type="button" className="btn" onClick={onExit}>Back to search</button>
          <span className="faint small">Ctrl+Enter runs it</span>
        </div>
        <details className="cql-help small">
          <summary>Fields, operators and functions</summary>
          <p>
            <b>Fields:</b> space, space.title, title, text, label, type (page, blogpost, folder), creator, contributor, mention, favourite, watcher, created,
            lastModified, id, parent, ancestor, status, property[Name].
          </p>
          <p><b>Operators:</b> = != ~ (contains, * and ? wildcards) !~ &lt; &lt;= &gt; &gt;= in (…) not in (…), combined with and, or, not and parentheses; order by title, created, lastModified or space (asc, desc).</p>
          <p><b>Functions:</b> currentUser(), now("-7d"), startOfDay(), startOfWeek("-1"), startOfMonth(), startOfYear(), endOfDay() …; dates as YYYY-MM-DD.</p>
          <p className="faint">Example: <code>{CQL_EXAMPLE}</code></p>
        </details>
      </form>
      <div style={{ marginTop: 16 }}>
        {results.isLoading && <Spinner />}
        {error && <div className="error-text" role="alert">{error}</div>}
        {results.data && <div className="faint small" style={{ marginBottom: 8 }}>{results.data.length} results</div>}
        {results.data?.map((r) => (
          <Link key={r.id} to={`/pages/${r.id}`} className="result" style={{ marginBottom: 4 }}>
            <div className="r-title">{r.title}</div>
            <div className="faint small">
              {r.spaceName}{r.kind === 'blog' ? ' · blog post' : ''} · updated {formatDate(r.updatedAt)}{r.updatedByName ? ` by ${r.updatedByName}` : ''}
              {r.labels.length > 0 && <> · {r.labels.join(', ')}</>}
            </div>
            {r.excerpt && <div className="r-snippet">{r.excerpt}</div>}
          </Link>
        ))}
      </div>
    </div>
  )
}

export default function SearchPage() {
  const [params, setParams] = useSearchParams()
  const cql = params.get('cql')
  const q = params.get('q') ?? ''
  const space = params.get('space') ?? ''
  const kind = params.get('kind') ?? ''
  const days = params.get('days') ?? ''
  const label = params.get('label') ?? ''
  const mine = params.get('mine') === '1'
  const me = useMe()
  /** changes one parameter and keeps the others */
  const setParam = (k: string, v: string) => {
    const n = new URLSearchParams(params)
    if (v) n.set(k, v)
    else n.delete(k)
    setParams(n)
  }
  const [input, setInput] = useState(q)
  const [labelInput, setLabelInput] = useState(label)
  const qc = useQueryClient()
  const saved = useQuery({ queryKey: ['saved-searches'], queryFn: () => api.get<{ id: string; name: string; params: string }[]>('/me/searches'), enabled: !!me.data?.user })
  const saveSearch = async () => {
    const name = window.prompt('Name this search', q)
    if (!name) return
    await api.post('/me/searches', { name, params: params.toString() })
    qc.invalidateQueries({ queryKey: ['saved-searches'] })
  }
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const ai = useQuery({ queryKey: ['ai-status'], queryFn: () => api.get<{ enabled: boolean }>('/ai/status'), staleTime: 300_000 })
  const [answer, setAnswer] = useState<{ loading: boolean; error?: string; data?: Answer; question: string } | null>(null)
  const ask = async () => {
    const question = input.trim()
    if (!question) return
    if (question !== q) setParam('q', question)
    setAnswer({ loading: true, question })
    try {
      const data = await api.post<Answer>('/ai/ask', { question, spaceKey: space || undefined })
      setAnswer({ loading: false, data, question })
    } catch (e) {
      setAnswer({ loading: false, error: e instanceof Error ? e.message : 'Could not get an answer', question })
    }
  }
  const filters = new URLSearchParams({ q, limit: '50' })
  if (space) filters.set('space', space)
  if (kind) filters.set('kind', kind)
  if (days) filters.set('days', days)
  if (label) filters.set('label', label)
  if (mine && me.data?.user) filters.set('contributor', me.data.user.id)
  const results = useQuery({
    queryKey: ['search-page', filters.toString()],
    queryFn: () => api.get<SearchHit[]>(`/search?${filters.toString()}`),
    enabled: !!q && cql === null,
  })
  if (cql !== null) {
    return <CqlSearch key={cql} initial={cql} onExit={() => setParams(new URLSearchParams(q ? { q } : {}))} onRun={(c) => setParams(new URLSearchParams({ cql: c }))} />
  }
  return (
    <div className="content-pad">
      <form
        className="row search-form"
        onSubmit={(e) => {
          e.preventDefault()
          setParam('q', input)
        }}
      >
        <input className="input" style={{ maxWidth: 520, height: 40, fontSize: 15 }} value={input} onChange={(e) => setInput(e.target.value)} placeholder="Search…" autoFocus />
        <select className="select" style={{ width: 200, height: 40 }} value={space} onChange={(e) => setParam('space', e.target.value)} aria-label="Space">
          <option value="">All spaces</option>
          {spaces.data?.map((s) => (
            <option key={s.key} value={s.key}>
              {s.name}
            </option>
          ))}
        </select>
        <button className="btn btn-primary" style={{ height: 40 }}>
          Search
        </button>
        {ai.data?.enabled && (
          <button type="button" className="btn" style={{ height: 40 }} disabled={!input.trim() || answer?.loading} onClick={ask} title="Answer the question from the pages you can see">
            <Sparkles size={15} /> Ask the wiki
          </button>
        )}
      </form>
      <div className="row search-filters">
        <select className="select" value={kind} onChange={(e) => setParam('kind', e.target.value)} aria-label="Type">
          <option value="">Pages and blog posts</option>
          <option value="page">Pages</option>
          <option value="blog">Blog posts</option>
        </select>
        <select className="select" value={days} onChange={(e) => setParam('days', e.target.value)} aria-label="Modified">
          <option value="">Modified any time</option>
          <option value="1">Modified today</option>
          <option value="7">In the last 7 days</option>
          <option value="30">In the last 30 days</option>
          <option value="365">In the last year</option>
        </select>
        <form onSubmit={(e) => { e.preventDefault(); setParam('label', labelInput.trim().toLowerCase()) }}>
          <input className="input" value={labelInput} onChange={(e) => setLabelInput(e.target.value)} onBlur={() => setParam('label', labelInput.trim().toLowerCase())} placeholder="Label" aria-label="Label" />
        </form>
        {me.data?.user && (
          <label className="row small">
            <input type="checkbox" checked={mine} onChange={(e) => setParam('mine', e.target.checked ? '1' : '')} /> Pages I worked on
          </label>
        )}
        <button type="button" className="btn btn-sm" onClick={() => setParams(new URLSearchParams({ cql: '' }))} title="Search with a query language: fields, operators and functions">
          <Braces size={14} /> Query
        </button>
        {me.data?.user && q && (
          <button type="button" className="btn btn-sm" onClick={saveSearch} title="Keep this search and its filters">
            <BookmarkPlus size={14} /> Save search
          </button>
        )}
      </div>
      {!!saved.data?.length && (
        <div className="row saved-searches">
          <span className="faint small">Saved:</span>
          {saved.data.map((s) => (
            <span key={s.id} className={`saved-search ${s.params === params.toString() ? 'active' : ''}`}>
              <Link to={`/search?${s.params}`} onClick={() => setInput(new URLSearchParams(s.params).get('q') ?? '')}>{s.name}</Link>
              <button
                type="button"
                title="Delete saved search"
                onClick={async () => {
                  await api.del(`/me/searches/${s.id}`)
                  qc.invalidateQueries({ queryKey: ['saved-searches'] })
                }}
              >
                <X size={12} />
              </button>
            </span>
          ))}
        </div>
      )}
      {answer && <AnswerCard state={answer} />}
      <div style={{ marginTop: 20 }}>
        {results.isLoading && <Spinner />}
        {results.data && <div className="faint small" style={{ marginBottom: 8 }}>{results.data.length} results</div>}
        {results.data?.map((h) => (
          <Link key={h.pageId} to={`/pages/${h.pageId}`} className="result" style={{ marginBottom: 4 }}>
            <div className="r-title">{h.title}</div>
            <div className="faint small">{h.spaceName}</div>
            {/* server-generated snippet: escaped text + <b> highlights */}
            <div className="r-snippet" dangerouslySetInnerHTML={{ __html: h.snippet }} />
          </Link>
        ))}
      </div>
    </div>
  )
}
