import { t } from '../i18n'
import { useState } from 'react'
import { Link } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { Link2 } from 'lucide-react'
import { api, type Backlink } from '../api'

/** The pages that link to this one (that the reader may open), below the page body. */
export function useBacklinks(pageId: string) {
  return useQuery({ queryKey: ['backlinks', pageId], queryFn: () => api.get<{ pages: Backlink[] }>(`/pages/${pageId}/backlinks`), staleTime: 30_000 })
}

const SHOWN = 6

export default function Backlinks({ pageId, spaceKey }: { pageId: string; spaceKey: string }) {
  const q = useBacklinks(pageId)
  const [all, setAll] = useState(false)
  const pages = q.data?.pages ?? []
  if (!pages.length) return null
  const shown = all ? pages : pages.slice(0, SHOWN)
  return (
    <div className="backlinks">
      <Link2 size={14} className="faint" aria-hidden />
      <span className="faint small">{t('Linked from')}</span>
      {shown.map((p) => (
        <Link
          key={p.id}
          to={`/pages/${p.id}`}
          className="backlink"
          title={p.via === 'include' ? t('Includes this page') : p.via === 'property' ? t('Refers to this page in a property') : undefined}
        >
          {p.title || t('Untitled')}
          {p.spaceKey !== spaceKey && <span className="faint">{' · '}{p.spaceName}</span>}
        </Link>
      ))}
      {pages.length > SHOWN && (
        <button className="btn btn-ghost btn-sm" onClick={() => setAll(!all)}>
          {all ? t('Show fewer') : t('{count} more', { count: pages.length - SHOWN })}
        </button>
      )}
    </div>
  )
}
