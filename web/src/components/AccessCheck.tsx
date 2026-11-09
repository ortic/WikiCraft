import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Check, Minus, SearchCheck, X } from 'lucide-react'
import { api } from '../api'
import { Spinner } from '../ui'

interface Explanation {
  user: { name: string; email: string | null } | null
  spaceRole: string
  access: { view: boolean; comment: boolean; edit: boolean; admin: boolean }
  steps: { ok: boolean | null; text: string }[]
}

/** "Why can (or can't) this person see this page?" */
export default function AccessCheck({ pageId, people }: { pageId: string; people: { id: string; name: string }[] }) {
  const [who, setWho] = useState('')
  const q = useQuery({ queryKey: ['access-check', pageId, who], queryFn: () => api.get<Explanation>(`/pages/${pageId}/access/${who}`), enabled: !!who })
  const verdict = (label: string, ok: boolean) => (
    <span className={`verdict ${ok ? 'yes' : 'no'}`}>{ok ? <Check size={13} /> : <X size={13} />} {label}</span>
  )
  return (
    <div className="access-check">
      <h4 className="row" style={{ margin: '0 0 8px' }}><SearchCheck size={15} /> Check someone's access</h4>
      <select className="select" value={who} onChange={(e) => setWho(e.target.value)}>
        <option value="">Choose a person…</option>
        <option value="anonymous">Anonymous visitor (not signed in)</option>
        {people.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
      </select>
      {who && (q.isLoading ? <Spinner /> : q.error ? <p className="error-text">{q.error instanceof Error ? q.error.message : 'Failed'}</p> : q.data && (
        <>
          <div className="row" style={{ margin: '10px 0', flexWrap: 'wrap', gap: 6 }}>
            {verdict('View', q.data.access.view)}
            {verdict('Comment', q.data.access.comment)}
            {verdict('Edit', q.data.access.edit)}
            {verdict('Manage', q.data.access.admin)}
          </div>
          <ol className="access-steps">
            {q.data.steps.map((s, i) => (
              <li key={i} className={s.ok === true ? 'yes' : s.ok === false ? 'no' : ''}>
                {s.ok === true ? <Check size={14} /> : s.ok === false ? <X size={14} /> : <Minus size={14} />}
                <span>{s.text}</span>
              </li>
            ))}
          </ol>
        </>
      ))}
    </div>
  )
}
