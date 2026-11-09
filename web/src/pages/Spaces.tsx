import { useState } from 'react'
import { Link, Navigate, useParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { Plus } from 'lucide-react'
import { api, type Space } from '../api'
import { ErrorBox, SpaceIcon, Spinner, useMe } from '../ui'
import CreateSpaceDialog from '../components/CreateSpaceDialog'

export default function SpacesPage() {
  const me = useMe()
  const [q, setQ] = useState('')
  const [create, setCreate] = useState(false)
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const list = (spaces.data ?? []).filter((s) => !q || s.name.toLowerCase().includes(q.toLowerCase()) || s.key.toLowerCase().includes(q.toLowerCase()))
  return (
    <div className="content-pad">
      <div className="row">
        <h1 style={{ margin: 0 }}>Spaces</h1>
        <span className="spacer" />
        {me.data?.user && me.data.user.role !== 'guest' && (
          <button className="btn btn-primary" onClick={() => setCreate(true)}>
            <Plus size={15} /> Create space
          </button>
        )}
      </div>
      <input className="input" style={{ margin: '18px 0', maxWidth: 360 }} placeholder="Filter spaces" value={q} onChange={(e) => setQ(e.target.value)} />
      <table className="list">
        <thead>
          <tr>
            <th>Space</th>
            <th>Key</th>
            <th>Pages</th>
            <th>Your role</th>
          </tr>
        </thead>
        <tbody>
          {list.map((s) => (
            <tr key={s.id}>
              <td>
                <Link to={`/spaces/${s.key}`} className="row" style={{ color: 'var(--text)' }}>
                  <SpaceIcon space={s} /> <b>{s.name}</b>
                </Link>
                {s.description && <div className="faint small" style={{ marginLeft: 32 }}>{s.description.slice(0, 140)}</div>}
              </td>
              <td><span className="badge">{s.key}</span></td>
              <td>{s.pageCount}</td>
              <td><span className="badge badge-accent">{s.role}</span></td>
            </tr>
          ))}
        </tbody>
      </table>
      {create && <CreateSpaceDialog onClose={() => setCreate(false)} />}
    </div>
  )
}

/** `/spaces/:key` → the space's home page. */
export function SpaceHome() {
  const { key } = useParams()
  const space = useQuery({ queryKey: ['space', key], queryFn: () => api.get<Space>(`/spaces/${key}`) })
  if (space.isLoading) return <div className="content-pad"><Spinner /></div>
  if (space.error) return <div className="content-pad"><ErrorBox error={space.error} /></div>
  if (space.data?.homePageId) return <Navigate to={`/pages/${space.data.homePageId}`} replace />
  return <div className="content-pad empty">This space has no home page yet.</div>
}
