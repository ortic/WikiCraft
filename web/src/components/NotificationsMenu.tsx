import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Bell, MessageSquare, AtSign, FilePen } from 'lucide-react'
import { api, formatDate } from '../api'

interface Notification {
  id: string
  kind: 'comment' | 'mention' | 'page_mention' | 'page_updated' | string
  pageId: string | null
  pageTitle: string | null
  actorName: string | null
  data: { excerpt?: string; version?: number; commentId?: string }
  createdAt: string
  readAt: string | null
}

function describe(n: Notification): string {
  const who = n.actorName ?? 'Someone'
  switch (n.kind) {
    case 'comment':
      return `${who} commented on ${n.pageTitle}`
    case 'mention':
      return `${who} mentioned you in a comment on ${n.pageTitle}`
    case 'page_mention':
      return `${who} mentioned you in ${n.pageTitle}`
    case 'page_updated':
      return `${who} updated ${n.pageTitle}${n.data.version ? ` (v${n.data.version})` : ''}`
    default:
      return `${who} · ${n.pageTitle ?? ''}`
  }
}

export default function NotificationsMenu() {
  const [open, setOpen] = useState(false)
  const qc = useQueryClient()
  const nav = useNavigate()
  const q = useQuery({ queryKey: ['notifications'], queryFn: () => api.get<{ unread: number; items: Notification[] }>('/notifications'), refetchInterval: 30_000 })
  const unread = q.data?.unread ?? 0
  const markAll = async () => {
    await api.post('/notifications/read', {})
    qc.invalidateQueries({ queryKey: ['notifications'] })
  }
  const openItem = async (n: Notification) => {
    setOpen(false)
    if (!n.readAt) {
      await api.post('/notifications/read', { ids: [n.id] })
      qc.invalidateQueries({ queryKey: ['notifications'] })
    }
    if (n.pageId) nav(`/pages/${n.pageId}`)
  }
  return (
    <div style={{ position: 'relative' }}>
      <button className="icon-btn" title="Notifications" onClick={() => setOpen((o) => !o)} style={{ position: 'relative' }}>
        <Bell size={16} />
        {unread > 0 && (
          <span style={{ position: 'absolute', top: 2, right: 2, minWidth: 15, height: 15, borderRadius: 8, background: 'var(--danger)', color: '#fff', fontSize: 10, fontWeight: 700, display: 'flex', alignItems: 'center', justifyContent: 'center', padding: '0 3px' }}>
            {unread > 9 ? '9+' : unread}
          </span>
        )}
      </button>
      {open && (
        <div className="dropdown menu" style={{ width: 380, maxHeight: 460 }} onMouseLeave={() => setOpen(false)}>
          <div className="row" style={{ padding: '6px 8px' }}>
            <b>Notifications</b>
            <span className="spacer" />
            {unread > 0 && (
              <button className="btn btn-ghost btn-sm" onClick={markAll}>
                Mark all read
              </button>
            )}
          </div>
          {q.data?.items.length === 0 && <div className="empty">You're all caught up.</div>}
          {q.data?.items.map((n) => (
            <div key={n.id} className="menu-item" onClick={() => openItem(n)} style={{ alignItems: 'flex-start', opacity: n.readAt ? 0.65 : 1 }}>
              <div className="mi-icon">{n.kind === 'page_updated' ? <FilePen size={15} /> : n.kind.includes('mention') ? <AtSign size={15} /> : <MessageSquare size={15} />}</div>
              <div style={{ minWidth: 0 }}>
                <div style={{ fontWeight: n.readAt ? 400 : 600 }}>{describe(n)}</div>
                {n.data.excerpt && <div className="mi-desc" style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{n.data.excerpt}</div>}
                <div className="mi-desc">{formatDate(n.createdAt)}</div>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
