// "N views" next to the page meta; for editors (when an admin turned on the viewer list) it
// opens who read the page and when.

import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { api, formatDateOnly } from '../api'
import { Avatar, Spinner } from '../ui'

interface Viewer {
  userId: string
  name: string
  lastViewed: string
  views: number
}

function when(day: string): string {
  const d = new Date(`${day}T00:00:00`)
  const today = new Date()
  today.setHours(0, 0, 0, 0)
  const days = Math.round((today.getTime() - d.getTime()) / 86_400_000)
  if (days <= 0) return 'today'
  if (days === 1) return 'yesterday'
  if (days < 30) return `${days} days ago`
  return formatDateOnly(day)
}

export function PageViews({ pageId, views, viewers, canSeeWho }: { pageId: string; views: number; viewers: number; canSeeWho: boolean }) {
  const [open, setOpen] = useState(false)
  const list = useQuery({ queryKey: ['viewers', pageId], queryFn: () => api.get<Viewer[]>(`/pages/${pageId}/viewers`), enabled: open })
  const label = `${views} view${views === 1 ? '' : 's'}`
  if (!canSeeWho) return <span title={`${viewers} people`}>{label}</span>
  return (
    <span className="page-views">
      <button type="button" className="link-btn page-views-btn" aria-haspopup="dialog" aria-expanded={open} title={`${viewers} people · see who`} onClick={() => setOpen((o) => !o)}>
        {label}
      </button>
      {open && (
        <div className="dropdown menu page-views-menu" role="dialog" aria-label="Who viewed this page" onMouseLeave={() => setOpen(false)}>
          <div className="menu-label">Viewed by {viewers} {viewers === 1 ? 'person' : 'people'}</div>
          {list.isLoading && <Spinner />}
          {list.data?.map((v) => (
            <div key={v.userId} className="row page-viewer">
              <Avatar name={v.name} id={v.userId} size={22} />
              <span className="page-viewer-name">{v.name}</span>
              <span className="spacer" />
              <span className="faint small">
                {when(v.lastViewed)} · {v.views}×
              </span>
            </div>
          ))}
          {list.data?.length === 0 && <div className="faint small" style={{ padding: '4px 8px' }}>Only anonymous visitors so far.</div>}
        </div>
      )}
    </span>
  )
}
