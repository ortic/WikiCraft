import { useState } from 'react'
import { Link, useSearchParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { api, type SearchHit, type Space } from '../api'
import { Spinner } from '../ui'

export default function SearchPage() {
  const [params, setParams] = useSearchParams()
  const q = params.get('q') ?? ''
  const space = params.get('space') ?? ''
  const [input, setInput] = useState(q)
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const results = useQuery({
    queryKey: ['search-page', q, space],
    queryFn: () => api.get<SearchHit[]>(`/search?q=${encodeURIComponent(q)}&limit=50${space ? `&space=${encodeURIComponent(space)}` : ''}`),
    enabled: !!q,
  })
  return (
    <div className="content-pad">
      <form
        className="row"
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
      </form>
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
