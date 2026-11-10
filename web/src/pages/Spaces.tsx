import { useState } from 'react'
import { Link, Navigate, useParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Plus, Star } from 'lucide-react'
import { api, type Space } from '../api'
import { ErrorBox, SpaceIcon, Spinner, useMe } from '../ui'
import CreateSpaceDialog from '../components/CreateSpaceDialog'

export default function SpacesPage() {
  const me = useMe()
  const [q, setQ] = useState('')
  const [create, setCreate] = useState(false)
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const [category, setCategory] = useState<string | null>(null)
  const [starredOnly, setStarredOnly] = useState(false)
  const qc = useQueryClient()
  const signedIn = !!me.data?.user
  const toggleStar = async (s: Space) => {
    await (s.starred ? api.del(`/spaces/${s.key}/star`) : api.put(`/spaces/${s.key}/star`))
    qc.setQueryData<Space[]>(['spaces'], (old) => old?.map((x) => (x.id === s.id ? { ...x, starred: !s.starred } : x)))
  }
  // starred first, then by name (the server sorts by name)
  const all = [...(spaces.data ?? [])].sort((a, b) => Number(!!b.starred) - Number(!!a.starred))
  const anyStarred = all.some((s) => s.starred)
  const categories = [...new Set(all.map((s) => s.category).filter((c): c is string => !!c))].sort((a, b) => a.localeCompare(b))
  const list = all.filter(
    (s) =>
      (!q || s.name.toLowerCase().includes(q.toLowerCase()) || s.key.toLowerCase().includes(q.toLowerCase())) &&
      (category === null || (s.category ?? '') === category) &&
      (!starredOnly || !!s.starred),
  )
  // grouped by category (uncategorised last) when there are categories and none is picked
  const groups: [string | null, Space[]][] =
    categories.length && category === null
      ? [...categories.map((c): [string | null, Space[]] => [c, list.filter((s) => s.category === c)]), ['Other', list.filter((s) => !s.category)]]
      : [[null, list]]
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
      <input className="input" style={{ margin: '18px 0 10px', maxWidth: 360 }} placeholder="Filter spaces" value={q} onChange={(e) => setQ(e.target.value)} />
      {(categories.length > 0 || anyStarred) && (
        <div className="chips" style={{ marginBottom: 14 }}>
          {anyStarred && (
            <button className={`chip${starredOnly ? ' active' : ''}`} aria-pressed={starredOnly} onClick={() => setStarredOnly((v) => !v)}>
              <Star size={12} style={{ verticalAlign: -1 }} /> Starred
            </button>
          )}
          {categories.length > 0 && <button className={`chip${category === null ? ' active' : ''}`} onClick={() => setCategory(null)}>All</button>}
          {categories.map((c) => (
            <button key={c} className={`chip${category === c ? ' active' : ''}`} onClick={() => setCategory(category === c ? null : c)}>{c}</button>
          ))}
          {categories.length > 0 && all.some((s) => !s.category) && (
            <button className={`chip${category === '' ? ' active' : ''}`} onClick={() => setCategory(category === '' ? null : '')}>Other</button>
          )}
        </div>
      )}
      <table className="list">
        <thead>
          <tr>
            <th>Space</th>
            <th>Key</th>
            <th>Pages</th>
            <th>Your role</th>
          </tr>
        </thead>
        {groups.filter(([, rows]) => rows.length).map(([title, rows]) => (
        <tbody key={title ?? ''}>
          {title !== null && (
            <tr className="list-group">
              <td colSpan={4}>{title}</td>
            </tr>
          )}
          {rows.map((s) => (
            <tr key={s.id}>
              <td>
                <div className="row" style={{ gap: 6 }}>
                  <Link to={`/spaces/${s.key}`} className="row" style={{ color: 'var(--text)' }}>
                    <SpaceIcon space={s} /> <b>{s.name}</b>
                  </Link>
                  {signedIn && (
                    <button
                      type="button"
                      className={`icon-btn star-toggle${s.starred ? ' on' : ''}`}
                      title={s.starred ? 'Unstar' : 'Star this space'}
                      aria-label={s.starred ? `Unstar ${s.name}` : `Star ${s.name}`}
                      aria-pressed={!!s.starred}
                      onClick={() => toggleStar(s)}
                    >
                      <Star size={14} fill={s.starred ? 'currentColor' : 'none'} />
                    </button>
                  )}
                </div>
                {s.description && <div className="faint small" style={{ marginLeft: 32 }}>{s.description.slice(0, 140)}</div>}
              </td>
              <td><span className="badge">{s.key}</span></td>
              <td>{s.pageCount}</td>
              <td><span className="badge badge-accent">{s.role}</span></td>
            </tr>
          ))}
        </tbody>
        ))}
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
