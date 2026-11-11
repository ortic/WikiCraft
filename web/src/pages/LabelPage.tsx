import { Link, useParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { Paperclip, Tag } from 'lucide-react'
import { api, formatBytes, formatDate, type PageListItem } from '../api'
import { Spinner } from '../ui'

export default function LabelPage() {
  const { name } = useParams()
  const pages = useQuery({ queryKey: ['label', name], queryFn: () => api.get<PageListItem[]>(`/labels/${encodeURIComponent(name!)}`) })
  const files = useQuery({
    queryKey: ['label-files', name],
    queryFn: () => api.get<{ id: string; filename: string; size: number; updatedAt: string; pageId: string; pageTitle: string; spaceName: string }[]>(`/labels/${encodeURIComponent(name!)}/attachments`),
  })
  return (
    <div className="content-pad">
      <h1 className="row">
        <Tag size={22} /> {name}
      </h1>
      {pages.isLoading && <Spinner />}
      <div className="page-list">
        {pages.data?.map((p) => (
          <Link key={p.id} to={`/pages/${p.id}`}>
            <div style={{ flex: 1 }}>
              <div style={{ fontWeight: 500 }}>{p.title}</div>
              <div className="faint small">{p.spaceName}</div>
            </div>
            <span className="faint small">{formatDate(p.updatedAt)}</span>
          </Link>
        ))}
      </div>
      {pages.data?.length === 0 && !files.data?.length && <div className="empty">No pages with this label.</div>}
      {!!files.data?.length && (
        <>
          <h2 className="row" style={{ fontSize: 17, marginTop: 28 }}><Paperclip size={16} /> Files</h2>
          <div>
            {files.data.map((f) => (
              <div key={f.id} className="row" style={{ gap: 10, padding: '8px 10px' }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <a href={`/api/attachments/${f.id}/content`} target="_blank" rel="noreferrer" style={{ fontWeight: 500 }}>{f.filename}</a>
                  <div className="faint small">
                    on <Link to={`/pages/${f.pageId}`}>{f.pageTitle}</Link> · {f.spaceName} · {formatBytes(f.size)}
                  </div>
                </div>
                <span className="faint small">{formatDate(f.updatedAt)}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  )
}
