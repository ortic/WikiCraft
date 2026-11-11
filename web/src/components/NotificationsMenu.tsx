import { t } from '../i18n'
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Bell, MessageSquare, AtSign, FilePen, ThumbsUp } from 'lucide-react'
import { api, formatDate } from '../api'
import { useToast } from '../ui'

interface Notification {
  id: string
  kind: 'comment' | 'mention' | 'page_mention' | 'page_updated' | string
  pageId: string | null
  pageTitle: string | null
  actorName: string | null
  data: { excerpt?: string; version?: number; commentId?: string; text?: string; due?: string; spaceKey?: string; spaceName?: string; requesterId?: string }
  createdAt: string
  readAt: string | null
}

function describe(n: Notification): string {
  const who = n.actorName ?? t('Someone')
  switch (n.kind) {
    case 'comment':
      return t('{who} commented on {pageTitle}', { who, pageTitle: n.pageTitle })
    case 'mention':
      return t('{who} mentioned you in a comment on {pageTitle}', { who, pageTitle: n.pageTitle })
    case 'page_mention':
      return t('{who} mentioned you in {pageTitle}', { who, pageTitle: n.pageTitle })
    case 'like':
      return `${who} liked ${n.pageTitle}`
    case 'access_request':
      return t('{who} asked for access to {pageTitle}', { who, pageTitle: n.pageTitle })
    case 'access_granted':
      return t('{who} gave you access to {pageTitle}', { who, pageTitle: n.pageTitle })
    case 'verification_request':
      return t('{who} asked you to review and verify {pageTitle}', { who, pageTitle: n.pageTitle })
    case 'page_created':
      return `${who} created ${n.pageTitle}`
    case 'page_updated':
      return `${who} updated ${n.pageTitle}${n.data.version ? ` (v${n.data.version})` : ''}`
    case 'task_due':
      return t('Your task is due: “{value}” on {pageTitle}', { value: n.data.text ?? '', pageTitle: n.pageTitle })
    case 'space_owner':
      return t('{who} made you the owner of the space {value}', { who, value: n.data.spaceName ?? n.data.spaceKey ?? '' })
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
  const toast = useToast()
  /** Answers an access request: read access to the page (and what it needs above), or no. */
  const decide = async (n: Notification, what: 'grant' | 'dismiss') => {
    try {
      const r = await api.post<{ granted?: boolean; hadAccess?: boolean; spaceAccess?: boolean; pagesAbove?: number } | undefined>(
        `/pages/${n.pageId}/access-${what}`,
        { userId: n.data.requesterId },
      )
      if (what === 'dismiss') toast(t('Request dismissed'))
      else if (r?.hadAccess) toast(t('{name} can already open it', { name: n.actorName ?? t('They') }))
      else {
        const name = n.actorName ?? t('They')
        const extra = [
          r?.spaceAccess ? t('can now view the space') : '',
          r?.pagesAbove ? t('also sees {count} restricted pages above it', { count: r.pagesAbove }) : '',
        ].filter(Boolean).join(t(' and '))
        toast(extra ? t('{name} can read {page} now (and {extra})', { name, page: n.pageTitle, extra }) : t('{name} can read {page} now', { name, page: n.pageTitle }))
      }
      qc.invalidateQueries({ queryKey: ['notifications'] })
    } catch (e) {
      toast(e instanceof Error ? e.message : t('That did not work'))
    }
  }
  const openItem = async (n: Notification) => {
    setOpen(false)
    if (!n.readAt) {
      await api.post('/notifications/read', { ids: [n.id] })
      qc.invalidateQueries({ queryKey: ['notifications'] })
    }
    if (n.pageId) nav(`/pages/${n.pageId}`)
    else if (n.data.spaceKey) nav(`/spaces/${n.data.spaceKey}`)
  }
  return (
    <div style={{ position: 'relative' }}>
      <button className="icon-btn" title={t('Notifications')} onClick={() => setOpen((o) => !o)} style={{ position: 'relative' }}>
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
            <b>{t('Notifications')}</b>
            <span className="spacer" />
            {unread > 0 && (
              <button className="btn btn-ghost btn-sm" onClick={markAll}>{t('Mark all read')}</button>
            )}
          </div>
          {q.data?.items.length === 0 && <div className="empty">{t('You\'re all caught up.')}</div>}
          {q.data?.items.map((n) => (
            <div key={n.id} className="menu-item" onClick={() => openItem(n)} style={{ alignItems: 'flex-start', opacity: n.readAt ? 0.65 : 1 }}>
              <div className="mi-icon">{n.kind === 'page_updated' ? <FilePen size={15} /> : n.kind === 'like' ? <ThumbsUp size={15} /> : n.kind.includes('mention') ? <AtSign size={15} /> : <MessageSquare size={15} />}</div>
              <div style={{ minWidth: 0 }}>
                <div style={{ fontWeight: n.readAt ? 400 : 600 }}>{describe(n)}</div>
                {n.data.excerpt && <div className="mi-desc" style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{n.data.excerpt}</div>}
                <div className="mi-desc">{formatDate(n.createdAt)}</div>
                {n.kind === 'access_request' && !n.readAt && n.pageId && n.data.requesterId && (
                  <div className="row" style={{ gap: 6, marginTop: 6 }} onClick={(e) => e.stopPropagation()}>
                    <button className="btn btn-primary btn-sm" onClick={() => decide(n, 'grant')}>{t('Grant access')}</button>
                    <button className="btn btn-sm" onClick={() => decide(n, 'dismiss')}>{t('Dismiss')}</button>
                  </div>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
