import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { FileText, Plus, Star, TrendingUp } from 'lucide-react'
import { api, formatDate, type PageListItem, type Space } from '../api'
import { SpaceIcon, useMe } from '../ui'
import CreateSpaceDialog from '../components/CreateSpaceDialog'

function PageList({ items, empty, timeLabel }: { items: PageListItem[] | undefined; empty: string; timeLabel: (p: PageListItem) => string }) {
  if (!items) return <div className="spinner" />
  if (!items.length) return <div className="empty">{empty}</div>
  return (
    <div className="page-list">
      {items.map((p) => (
        <Link key={p.id} to={`/pages/${p.id}`}>
          <FileText size={16} className="faint" />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontWeight: 500, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{p.title}</div>
            <div className="faint small">{p.spaceName}</div>
          </div>
          <span className="faint small">{timeLabel(p)}</span>
        </Link>
      ))}
    </div>
  )
}

export default function Home() {
  const me = useMe()
  const [tab, setTab] = useState<'viewed' | 'updated' | 'popular' | 'starred'>('viewed')
  const [create, setCreate] = useState(false)
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const recent = useQuery({ queryKey: ['recent'], queryFn: () => api.get<{ viewed: PageListItem[]; updated: PageListItem[] }>('/recent'), enabled: !!me.data?.user })
  const favs = useQuery({ queryKey: ['favorites'], queryFn: () => api.get<PageListItem[]>('/favorites'), enabled: !!me.data?.user })
  const popular = useQuery({
    queryKey: ['popular'],
    queryFn: () => api.get<(PageListItem & { views: number; viewers: number })[]>('/popular?days=30'),
    enabled: !!me.data?.user && tab === 'popular',
  })
  const name = me.data?.user?.name?.split(' ')[0]
  return (
    <div className="content-pad">
      <h1 style={{ margin: '4px 0 2px' }}>{name ? `Good to see you, ${name}` : me.data?.workspace?.name}</h1>
      <p className="muted" style={{ marginTop: 0 }}>{me.data?.workspace?.name}</p>

      <div className="row" style={{ marginTop: 28 }}>
        <h3 style={{ margin: 0 }}>Spaces</h3>
        <span className="spacer" />
        {me.data?.user && me.data.user.role !== 'guest' && (
          <button className="btn btn-sm" onClick={() => setCreate(true)}>
            <Plus size={14} /> New space
          </button>
        )}
      </div>
      <div className="grid-cards" style={{ marginTop: 12 }}>
        {spaces.data?.map((s) => (
          <Link key={s.id} to={`/spaces/${s.key}`} className="space-card">
            <SpaceIcon space={s} size={36} />
            <div style={{ minWidth: 0 }}>
              <div style={{ fontWeight: 600 }}>{s.name}</div>
              <div className="faint small">
                {s.key} · {s.pageCount} pages
              </div>
            </div>
          </Link>
        ))}
        {spaces.data?.length === 0 && <div className="empty">No spaces yet — create the first one or import from Confluence in Admin.</div>}
      </div>

      {me.data?.user && (
        <>
          <div className="tabs" style={{ marginTop: 32 }}>
            <button className={`tab ${tab === 'viewed' ? 'active' : ''}`} onClick={() => setTab('viewed')}>
              Recently viewed
            </button>
            <button className={`tab ${tab === 'updated' ? 'active' : ''}`} onClick={() => setTab('updated')}>
              Recently updated
            </button>
            <button className={`tab ${tab === 'popular' ? 'active' : ''}`} onClick={() => setTab('popular')}>
              <TrendingUp size={13} /> Popular
            </button>
            <button className={`tab ${tab === 'starred' ? 'active' : ''}`} onClick={() => setTab('starred')}>
              <Star size={13} /> Starred
            </button>
          </div>
          {tab === 'viewed' && <PageList items={recent.data?.viewed} empty="Pages you visit show up here." timeLabel={(p) => formatDate(p.at)} />}
          {tab === 'updated' && <PageList items={recent.data?.updated} empty="Nothing updated yet." timeLabel={(p) => `${p.updatedByName ?? ''} · ${formatDate(p.updatedAt)}`} />}
          {tab === 'popular' && (
            <PageList
              items={popular.data}
              empty="Pages people read most in the last 30 days show up here."
              timeLabel={(p) => {
                const x = p as PageListItem & { viewers: number; views: number }
                return `${x.viewers} ${x.viewers === 1 ? 'reader' : 'readers'} · ${x.views} views`
              }}
            />
          )}
          {tab === 'starred' && <PageList items={favs.data} empty="Star pages to find them quickly." timeLabel={(p) => formatDate(p.updatedAt)} />}
        </>
      )}
      {create && <CreateSpaceDialog onClose={() => setCreate(false)} />}
    </div>
  )
}
