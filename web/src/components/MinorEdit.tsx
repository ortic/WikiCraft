import { t } from '../i18n'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { BellOff } from 'lucide-react'
import { api } from '../api'
import { useToast } from '../ui'

/** "Minor change" switch while editing: watchers aren't notified about this session's version
 * when everybody who edited it switched it on. */
export default function MinorEditToggle({ pageId }: { pageId: string }) {
  const qc = useQueryClient()
  const toast = useToast()
  const state = useQuery({ queryKey: ['minor-edit', pageId], queryFn: () => api.get<{ minor: boolean }>(`/pages/${pageId}/minor-edit`) })
  const minor = !!state.data?.minor
  const toggle = async () => {
    try {
      qc.setQueryData(['minor-edit', pageId], await api.put<{ minor: boolean }>(`/pages/${pageId}/minor-edit`, { minor: !minor }))
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Could not change that'))
    }
  }
  return (
    <button
      type="button"
      className={`badge minor-toggle${minor ? ' on' : ''}`}
      aria-pressed={minor}
      title={minor ? t('Your changes are minor: watchers aren’t notified about them. Click to notify them again.') : t('Mark your changes as minor: watchers aren’t notified')}
      onClick={toggle}
    >
      <BellOff size={12} /> {minor ? t('Minor change: no notifications') : t('Minor change')}
    </button>
  )
}
