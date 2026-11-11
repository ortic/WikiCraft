import { t } from '../i18n'
import { useEffect } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { FilePlus2 } from 'lucide-react'
import { api, can, type Space } from '../api'
import { Spinner } from '../ui'

/**
 * Target of links to pages that didn't exist at import time (`/missing?space=KEY&title=T`) or of
 * old Confluence ids (`/missing?source=confluence:123`). Resolves to the page if it exists now,
 * otherwise offers to create it.
 */
export default function Missing(props: { space?: string; title?: string } = {}) {
  const [params] = useSearchParams()
  const nav = useNavigate()
  const qc = useQueryClient()
  // a readable address (`/wiki/KEY/slug`) that leads nowhere passes its parts in
  const space = props.space ?? params.get('space')
  const title = props.title ?? params.get('title')
  const source = props.space ? null : params.get('source')
  const qs = source ? `source=${encodeURIComponent(source)}` : `space=${encodeURIComponent(space ?? '')}&title=${encodeURIComponent(title ?? '')}`
  const resolved = useQuery({ queryKey: ['resolve', qs], queryFn: () => api.get<{ pageId: string | null }>(`/resolve?${qs}`), retry: false })
  const spaceInfo = useQuery({ queryKey: ['space', space], queryFn: () => api.get<Space>(`/spaces/${space}`), enabled: !!space, retry: false })
  useEffect(() => {
    if (resolved.data?.pageId) nav(`/pages/${resolved.data.pageId}`, { replace: true })
  }, [resolved.data, nav])
  if (resolved.isLoading || resolved.data?.pageId) return <div className="content-pad"><Spinner /></div>
  const canCreate = !!space && !!title && spaceInfo.data && ['editor', 'admin'].includes(spaceInfo.data.role) && can(spaceInfo.data, 'page.add')
  return (
    <div className="content-pad" style={{ textAlign: 'center', paddingTop: 80 }}>
      <FilePlus2 size={44} className="faint" />
      <h2>{title ? t('“{title}” doesn\'t exist yet', { title }) : t('Page not found')}</h2>
      <p className="muted">{source ? t('This link points to a Confluence page that was not part of any import.') : space ? t('There is no page with this title in {space}.', { space }) : ''}</p>
      {canCreate && (
        <button
          className="btn btn-primary"
          onClick={async () => {
            const r = await api.post<{ id: string }>('/pages', { spaceKey: space, title })
            qc.invalidateQueries({ queryKey: ['tree'] })
            nav(`/pages/${r.id}`, { replace: true })
          }}
        >{t('Create “{title}”', { title })}</button>
      )}
    </div>
  )
}
