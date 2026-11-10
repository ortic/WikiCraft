import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { Activity, FileText, ListChecks, MessageSquare, PencilLine, Plus, Star, TrendingUp, FilePlus2 } from 'lucide-react'
import { api, formatDate, type PageListItem, type Space } from '../api'
import { Avatar, SpaceIcon, useMe } from '../ui'
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

interface ActivityEvent {
  kind: 'created' | 'edited' | 'commented'
  pageId: string
  pageTitle: string
  spaceKey: string
  spaceName: string
  actorId: string | null
  actorName: string | null
  at: string
  detail: string | null
  count: number
}

/** Who created, edited or commented on what, across the spaces you can see (or what you watch). */
function ActivityFeed() {
  const [scope, setScope] = useState<'all' | 'watching'>('all')
  const q = useQuery({ queryKey: ['activity', scope], queryFn: () => api.get<ActivityEvent[]>(`/activity?scope=${scope}&limit=40`) })
  const verb = (e: ActivityEvent) =>
    e.kind === 'created' ? 'created' : e.kind === 'commented' ? 'commented on' : e.count > 1 ? `edited (${e.count} versions)` : 'edited'
  const Icon = ({ kind }: { kind: ActivityEvent['kind'] }) =>
    kind === 'created' ? <FilePlus2 size={13} /> : kind === 'commented' ? <MessageSquare size={13} /> : <PencilLine size={13} />
  return (
    <div>
      <div className="chips" style={{ margin: '4px 0 12px' }}>
        <button className={`chip${scope === 'all' ? ' active' : ''}`} onClick={() => setScope('all')}>Everything</button>
        <button className={`chip${scope === 'watching' ? ' active' : ''}`} onClick={() => setScope('watching')}>Pages and spaces I watch</button>
      </div>
      {!q.data ? (
        <div className="spinner" />
      ) : !q.data.length ? (
        <div className="empty">{scope === 'watching' ? 'Watch pages or spaces to follow their changes here.' : 'Nothing has happened yet.'}</div>
      ) : (
        <ul className="activity">
          {q.data.map((e, i) => (
            <li key={`${e.pageId}-${e.at}-${i}`}>
              <Avatar name={e.actorName ?? '?'} id={e.actorId ?? undefined} size={28} />
              <div className="activity-body">
                <div>
                  <b>{e.actorName ?? 'Someone'}</b> <span className="activity-verb"><Icon kind={e.kind} /> {verb(e)}</span>{' '}
                  <Link to={`/pages/${e.pageId}`}>{e.pageTitle}</Link>
                </div>
                {e.detail && <div className="activity-detail">{e.detail}</div>}
                <div className="faint small">{e.spaceName} · {formatDate(e.at)}</div>
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

export default function Home() {
  const me = useMe()
  const [tab, setTab] = useState<'viewed' | 'updated' | 'activity' | 'popular' | 'starred' | 'drafts'>('viewed')
  const [create, setCreate] = useState(false)
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const recent = useQuery({ queryKey: ['recent'], queryFn: () => api.get<{ viewed: PageListItem[]; updated: PageListItem[] }>('/recent'), enabled: !!me.data?.user })
  const favs = useQuery({ queryKey: ['favorites'], queryFn: () => api.get<PageListItem[]>('/favorites'), enabled: !!me.data?.user })
  const drafts = useQuery({ queryKey: ['drafts'], queryFn: () => api.get<PageListItem[]>('/me/drafts'), enabled: !!me.data?.user })
  const popular = useQuery({
    queryKey: ['popular'],
    queryFn: () => api.get<(PageListItem & { views: number; viewers: number })[]>('/popular?days=30'),
    enabled: !!me.data?.user && tab === 'popular',
  })
  const tasks = useQuery({
    queryKey: ['tasks', 'open'],
    queryFn: () => api.get<{ pageId: string; idx: number; text: string; due: string | null; pageTitle: string }[]>('/tasks?status=open'),
    enabled: !!me.data?.user,
  })
  const name = me.data?.user?.name?.split(' ')[0]
  const today = new Date().toISOString().slice(0, 10)
  return (
    <div className="content-pad">
      <h1 style={{ margin: '4px 0 2px' }}>{name ? `Good to see you, ${name}` : me.data?.workspace?.name}</h1>
      <p className="muted" style={{ marginTop: 0 }}>{me.data?.workspace?.name}</p>

      {!!tasks.data?.length && (
        <div className="card home-tasks">
          <div className="row">
            <ListChecks size={16} /> <b>Your open tasks</b>
            <span className="faint small">{tasks.data.length}</span>
            <span className="spacer" />
            <Link to="/tasks" className="small">All tasks</Link>
          </div>
          {tasks.data.slice(0, 5).map((t) => (
            <Link key={`${t.pageId}-${t.idx}`} to={`/pages/${t.pageId}`} className="home-task">
              <span className="home-task-text">{t.text || 'Untitled task'}</span>
              <span className="faint small">{t.pageTitle}</span>
              {t.due && <span className={`badge ${t.due < today ? 'badge-danger' : t.due === today ? 'badge-warning' : ''}`}>{t.due < today ? 'overdue ' : 'due '}{t.due}</span>}
            </Link>
          ))}
        </div>
      )}

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
            <button className={`tab ${tab === 'activity' ? 'active' : ''}`} onClick={() => setTab('activity')}>
              <Activity size={13} /> Activity
            </button>
            <button className={`tab ${tab === 'popular' ? 'active' : ''}`} onClick={() => setTab('popular')}>
              <TrendingUp size={13} /> Popular
            </button>
            <button className={`tab ${tab === 'starred' ? 'active' : ''}`} onClick={() => setTab('starred')}>
              <Star size={13} /> Starred
            </button>
            {!!drafts.data?.length && (
              <button className={`tab ${tab === 'drafts' ? 'active' : ''}`} onClick={() => setTab('drafts')}>
                <PencilLine size={13} /> Drafts ({drafts.data.length})
              </button>
            )}
          </div>
          {tab === 'viewed' && <PageList items={recent.data?.viewed} empty="Pages you visit show up here." timeLabel={(p) => formatDate(p.at)} />}
          {tab === 'updated' && <PageList items={recent.data?.updated} empty="Nothing updated yet." timeLabel={(p) => `${p.updatedByName ?? ''} · ${formatDate(p.updatedAt)}`} />}
          {tab === 'activity' && <ActivityFeed />}
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
          {tab === 'drafts' && <PageList items={drafts.data} empty="No unpublished drafts." timeLabel={(p) => (p.publishAt ? `publishes ${new Date(p.publishAt).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })}` : `edited ${formatDate(p.updatedAt)}`)} />}
        </>
      )}
      {create && <CreateSpaceDialog onClose={() => setCreate(false)} />}
    </div>
  )
}
