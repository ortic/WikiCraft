import { useEffect } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { FilePlus2 } from 'lucide-react'
import { api, type Space } from '../api'
import { Spinner } from '../ui'

/**
 * Target of links to pages that didn't exist at import time (`/missing?space=KEY&title=T`) or of
 * old Confluence ids (`/missing?source=confluence:123`). Resolves to the page if it exists now,
 * otherwise offers to create it.
 */
export default function Missing() {
  const [params] = useSearchParams()
  const nav = useNavigate()
  const qc = useQueryClient()
  const space = params.get('space')
  const title = params.get('title')
  const source = params.get('source')
  const qs = source ? `source=${encodeURIComponent(source)}` : `space=${encodeURIComponent(space ?? '')}&title=${encodeURIComponent(title ?? '')}`
  const resolved = useQuery({ queryKey: ['resolve', qs], queryFn: () => api.get<{ pageId: string | null }>(`/resolve?${qs}`), retry: false })
  const spaceInfo = useQuery({ queryKey: ['space', space], queryFn: () => api.get<Space>(`/spaces/${space}`), enabled: !!space, retry: false })
  useEffect(() => {
    if (resolved.data?.pageId) nav(`/pages/${resolved.data.pageId}`, { replace: true })
  }, [resolved.data, nav])
  if (resolved.isLoading || resolved.data?.pageId) return <div className="content-pad"><Spinner /></div>
  const canCreate = !!space && !!title && spaceInfo.data && ['editor', 'admin'].includes(spaceInfo.data.role)
  return (
    <div className="content-pad" style={{ textAlign: 'center', paddingTop: 80 }}>
      <FilePlus2 size={44} className="faint" />
      <h2>{title ? `“${title}” doesn't exist yet` : 'Page not found'}</h2>
      <p className="muted">{source ? 'This link points to a Confluence page that was not part of any import.' : space ? `There is no page with this title in ${space}.` : ''}</p>
      {canCreate && (
        <button
          className="btn btn-primary"
          onClick={async () => {
            const r = await api.post<{ id: string }>('/pages', { spaceKey: space, title })
            qc.invalidateQueries({ queryKey: ['tree'] })
            nav(`/pages/${r.id}`, { replace: true })
          }}
        >
          Create “{title}”
        </button>
      )}
    </div>
  )
}
