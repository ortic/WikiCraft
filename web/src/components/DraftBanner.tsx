import { t } from '../i18n'
import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { CalendarClock } from 'lucide-react'
import { api, formatDateTime } from '../api'
import { useToast } from '../ui'

/** A date-time as the `datetime-local` input wants it: local time, no seconds. */
function toLocalInput(d: Date) {
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** Banner on your unpublished draft: publish now, or let it publish itself at a time. */
export default function DraftBanner({ page, onPublish }: { page: { id: string; kind: string; publishAt?: string | null }; onPublish: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const [editing, setEditing] = useState(false)
  const [when, setWhen] = useState(() => toLocalInput(page.publishAt ? new Date(page.publishAt) : new Date(Date.now() + 24 * 3600 * 1000)))
  const scheduled = page.publishAt ? new Date(page.publishAt) : null
  const what = page.kind === 'blog' ? 'post' : 'page'
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['page', page.id] })
    qc.invalidateQueries({ queryKey: ['drafts'] })
  }
  const save = async () => {
    const at = new Date(when)
    if (Number.isNaN(at.getTime())) return toast(t('Pick a date and time'))
    try {
      await api.put(`/pages/${page.id}/publish-schedule`, { at: at.toISOString() })
      setEditing(false)
      refresh()
      toast(t('This {what} publishes itself on {date}', { what, date: formatDateTime(at) }))
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Could not schedule it'))
    }
  }
  const cancel = async () => {
    await api.del(`/pages/${page.id}/publish-schedule`)
    refresh()
    toast(t('Not scheduled any more: it stays a draft until you publish it'))
  }
  return (
    <div className="draft-banner" role="status">
      <b>{t('Draft')}</b>
      {scheduled ? (
        <span className="row" style={{ gap: 6 }}>
          <CalendarClock size={15} />{' '}{t('Publishes itself on {date}. Until then only you see it.', { date: formatDateTime(scheduled) })}</span>
      ) : (
        <span>{t('Only you can see this {what} and the pages below it. Publish it when it\'s ready.', { what })}</span>
      )}
      <span className="spacer" />
      {editing ? (
        <span className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
          <input
            type="datetime-local"
            className="input input-sm"
            aria-label={t('Publish on')}
            value={when}
            min={toLocalInput(new Date(Date.now() + 60_000))}
            onChange={(e) => setWhen(e.target.value)}
          />
          <button className="btn btn-primary btn-sm" onClick={save}>{t('Schedule')}</button>
          <button className="btn btn-sm" onClick={() => setEditing(false)}>{t('Cancel')}</button>
        </span>
      ) : (
        <>
          <button className="btn btn-sm" onClick={() => setEditing(true)}>
            <CalendarClock size={14} /> {scheduled ? t('Change time') : t('Schedule…')}
          </button>
          {scheduled && <button className="btn btn-sm" onClick={cancel}>{t('Don\'t schedule')}</button>}
          <button className="btn btn-primary btn-sm" onClick={onPublish}>{scheduled ? t('Publish now') : t('Publish')}</button>
        </>
      )}
    </div>
  )
}
