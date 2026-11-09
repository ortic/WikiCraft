import { Link, useParams } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { Tag } from 'lucide-react'
import { api, formatDate, type PageListItem } from '../api'
import { Spinner } from '../ui'

export default function LabelPage() {
  const { name } = useParams()
  const pages = useQuery({ queryKey: ['label', name], queryFn: () => api.get<PageListItem[]>(`/labels/${encodeURIComponent(name!)}`) })
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
      {pages.data?.length === 0 && <div className="empty">No pages with this label.</div>}
    </div>
  )
}
