import { useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { FileText, Sparkles } from 'lucide-react'
import { api, type SearchHit, type Space } from '../api'
import { Spinner } from '../ui'

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

export default function SearchPage() {
  const [params, setParams] = useSearchParams()
  const q = params.get('q') ?? ''
  const space = params.get('space') ?? ''
  const [input, setInput] = useState(q)
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const ai = useQuery({ queryKey: ['ai-status'], queryFn: () => api.get<{ enabled: boolean }>('/ai/status'), staleTime: 300_000 })
  const [answer, setAnswer] = useState<{ loading: boolean; error?: string; data?: Answer; question: string } | null>(null)
  const ask = async () => {
    const question = input.trim()
    if (!question) return
    if (question !== q) setParams({ q: question, ...(space ? { space } : {}) })
    setAnswer({ loading: true, question })
    try {
      const data = await api.post<Answer>('/ai/ask', { question, spaceKey: space || undefined })
      setAnswer({ loading: false, data, question })
    } catch (e) {
      setAnswer({ loading: false, error: e instanceof Error ? e.message : 'Could not get an answer', question })
    }
  }
  const results = useQuery({
    queryKey: ['search-page', q, space],
    queryFn: () => api.get<SearchHit[]>(`/search?q=${encodeURIComponent(q)}&limit=50${space ? `&space=${encodeURIComponent(space)}` : ''}`),
    enabled: !!q,
  })
  return (
    <div className="content-pad">
      <form
        className="row search-form"
        onSubmit={(e) => {
          e.preventDefault()
          setParams({ q: input, ...(space ? { space } : {}) })
        }}
      >
        <input className="input" style={{ maxWidth: 520, height: 40, fontSize: 15 }} value={input} onChange={(e) => setInput(e.target.value)} placeholder="Search…" autoFocus />
        <select className="select" style={{ width: 200, height: 40 }} value={space} onChange={(e) => setParams({ q, ...(e.target.value ? { space: e.target.value } : {}) })}>
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
