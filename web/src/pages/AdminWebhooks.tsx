import { t } from '../i18n'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { CheckCircle2, Copy, Plus, Send, Trash2, Webhook, XCircle } from 'lucide-react'
import { api, formatDate } from '../api'
import { Modal, Spinner, useToast } from '../ui'

interface Hook {
  id: string
  name: string
  url: string
  events: string[]
  spaceKey: string | null
  format?: string
  active: boolean
  failureCount: number
  lastDeliveryAt: string | null
  lastStatus: number | null
  lastError: string | null
}

interface Delivery {
  id: string
  event: string
  createdAt: string
  attempts: number
  deliveredAt: string | null
  failedAt: string | null
  nextAttemptAt: string
  responseStatus: number | null
  lastError: string | null
  durationMs: number | null
}

/** Admin → Webhooks */
export default function AdminWebhooks() {
  const qc = useQueryClient()
  const toast = useToast()
  const hooks = useQuery({ queryKey: ['webhooks'], queryFn: () => api.get<{ hooks: Hook[]; events: string[] }>('/admin/webhooks'), refetchInterval: 5000 })
  const [creating, setCreating] = useState(false)
  const [open, setOpen] = useState<string | null>(null)
  const refresh = () => qc.invalidateQueries({ queryKey: ['webhooks'] })
  const act = async (fn: () => Promise<unknown>, msg?: string) => {
    try {
      await fn()
      refresh()
      if (msg) toast(msg)
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Failed'))
    }
  }
  return (
    <div>
      <div className="row" style={{ marginBottom: 12 }}>
        <p className="muted" style={{ margin: 0, maxWidth: 680 }}>{t('WikiCraft POSTs a JSON event to your endpoint when something happens. Each request is signed: verify the')}{' '}<code>X-WikiCraft-Signature</code>{' '}{t('header (')}<code>sha256=</code>{' '}{t('HMAC of the body with the hook\'s secret). Failed deliveries are retried for about 40 minutes.')}</p>
        <span className="spacer" />
        <button className="btn btn-primary" onClick={() => setCreating(true)}><Plus size={15} />{' '}{t('Add webhook')}</button>
      </div>
      {hooks.isLoading && <Spinner />}
      {hooks.data?.hooks.length === 0 && <div className="empty">{t('No webhooks yet.')}</div>}
      {hooks.data?.hooks.map((h) => (
        <div key={h.id} className="card" style={{ marginBottom: 10 }}>
          <div className="row" style={{ flexWrap: 'wrap' }}>
            <Webhook size={16} />
            <b>{h.name || new URL(h.url).host}</b>
            <code className="small" style={{ overflow: 'hidden', textOverflow: 'ellipsis', maxWidth: 360 }}>{h.url}</code>
            {!h.active ? <span className="badge">{t('paused')}</span> : h.lastStatus && h.lastStatus < 300 ? <span className="badge badge-accent">{t('healthy')}</span> : h.failureCount > 0 ? <span className="badge badge-danger">{t('failing')}</span> : null}
            <span className="spacer" />
            <button className="btn small" onClick={() => act(() => api.post(`/admin/webhooks/${h.id}/test`), t('Test event queued'))}><Send size={13} />{' '}{t('Send test')}</button>
            <button className="btn small" onClick={() => act(() => api.patch(`/admin/webhooks/${h.id}`, { active: !h.active }))}>{h.active ? t('Pause') : t('Resume')}</button>
            <button className="btn small" onClick={() => setOpen(open === h.id ? null : h.id)}>{t('Deliveries')}</button>
            <button className="btn btn-ghost small" title={t('Delete')} onClick={() => confirm(t('Delete this webhook?')) && act(() => api.del(`/admin/webhooks/${h.id}`), t('Webhook deleted'))}><Trash2 size={14} /></button>
          </div>
          <div className="faint small" style={{ marginTop: 6 }}>
            {h.format && h.format !== 'json' ? `${FORMATS[h.format] ?? h.format} · ` : ''}{h.events.length ? h.events.join(', ') : t('all events')} · {h.spaceKey ? t('space {spaceKey}', { spaceKey: h.spaceKey }) : t('all spaces')}
            {h.lastDeliveryAt ? ' · ' + (h.lastStatus ? t('last delivery {date} (HTTP {status})', { date: formatDate(h.lastDeliveryAt), status: h.lastStatus }) : t('last delivery {date}', { date: formatDate(h.lastDeliveryAt) })) : ''}
            {h.lastError && h.failureCount > 0 ? ` · ${h.lastError}` : ''}
          </div>
          {open === h.id && <Deliveries hookId={h.id} />}
        </div>
      ))}
      {creating && hooks.data && <CreateHook events={hooks.data.events} onClose={() => { setCreating(false); refresh() }} />}
    </div>
  )
}

function Deliveries({ hookId }: { hookId: string }) {
  const q = useQuery({ queryKey: ['webhook-deliveries', hookId], queryFn: () => api.get<Delivery[]>(`/admin/webhooks/${hookId}/deliveries`), refetchInterval: 3000 })
  if (q.isLoading) return <Spinner />
  if (!q.data?.length) return <p className="faint small">{t('No deliveries yet.')}</p>
  return (
    <table className="list" style={{ marginTop: 10 }}>
      <thead><tr><th>{t('Event')}</th><th>{t('When')}</th><th>{t('Result')}</th><th>{t('Attempts')}</th><th>{t('Time')}</th></tr></thead>
      <tbody>
        {q.data.map((d) => (
          <tr key={d.id}>
            <td><code>{d.event}</code></td>
            <td className="faint small">{formatDate(d.createdAt)}</td>
            <td className="small">
              {d.deliveredAt ? <span className="row" style={{ gap: 4 }}><CheckCircle2 size={14} color="var(--success)" />{' '}{t('HTTP {responseStatus}', { responseStatus: d.responseStatus })}</span>
                : d.failedAt ? <span className="row" style={{ gap: 4 }}><XCircle size={14} color="var(--danger)" />{' '}{t('gave up: {lastError}', { lastError: d.lastError })}</span>
                : d.attempts > 0 ? <span title={d.lastError ?? ''}>{t('retrying {date} · {lastError}', { date: formatDate(d.nextAttemptAt), lastError: d.lastError })}</span>
                : t('queued')}
            </td>
            <td>{d.attempts}</td>
            <td className="faint small">{d.durationMs != null ? t('{durationMs} ms', { durationMs: d.durationMs }) : ''}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

const FORMATS: Record<string, string> = {
  json: t('Any endpoint (WikiCraft JSON, signed)'),
  slack: t('Slack or Mattermost channel'),
  teams: t('Microsoft Teams channel (Workflows)'),
  googlechat: t('Google Chat space'),
}

function CreateHook({ events, onClose }: { events: string[]; onClose: () => void }) {
  const [form, setForm] = useState({ name: '', url: 'https://', spaceKey: '', format: 'json' })
  const [selected, setSelected] = useState<string[]>([])
  const [error, setError] = useState<string | null>(null)
  const [secret, setSecret] = useState<string | null>(null)
  const submit = async () => {
    setError(null)
    try {
      const r = await api.post<{ secret: string }>('/admin/webhooks', { ...form, events: selected })
      setSecret(r.secret)
    } catch (e) {
      setError(e instanceof Error ? e.message : t('Failed'))
    }
  }
  if (secret)
    return (
      <Modal title={t('Webhook created')} onClose={onClose} footer={<button className="btn btn-primary" onClick={onClose}>{t('Done')}</button>}>
        <p style={{ marginTop: 0 }}>{t('Copy the signing secret now — it isn\'t shown again.')}</p>
        <div className="row">
          <input className="input" readOnly value={secret} onFocus={(e) => e.target.select()} />
          <button className="btn" onClick={() => navigator.clipboard?.writeText(secret)}><Copy size={14} />{' '}{t('Copy')}</button>
        </div>
      </Modal>
    )
  return (
    <Modal title={t('Add webhook')} onClose={onClose} footer={<><button className="btn" onClick={onClose}>{t('Cancel')}</button><button className="btn btn-primary" onClick={submit}>{t('Create')}</button></>}>
      <div className="field"><label>{t('Name')}</label><input className="input" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} placeholder={t('e.g. Slack relay')} /></div>
      <div className="field">
        <label>{t('Send to')}</label>
        <select className="select" value={form.format} onChange={(e) => setForm({ ...form, format: e.target.value })}>
          {Object.entries(FORMATS).map(([k, v]) => <option key={k} value={k}>{v}</option>)}
        </select>
        {form.format !== 'json' && <div className="faint small">{t('Paste the channel’s incoming-webhook URL; events arrive as readable messages with a link to the page.')}</div>}
      </div>
      <div className="field"><label>{t('Endpoint URL')}</label><input className="input" value={form.url} onChange={(e) => setForm({ ...form, url: e.target.value })} autoFocus /></div>
      <div className="field">
        <label>{t('Events')}</label>
        <div className="event-grid">
          {events.map((e) => (
            <label key={e} className="row small">
              <input type="checkbox" checked={selected.includes(e)} onChange={(ev) => setSelected(ev.target.checked ? [...selected, e] : selected.filter((x) => x !== e))} /> <code>{e}</code>
            </label>
          ))}
        </div>
        <div className="faint small">{t('None selected = all events.')}</div>
      </div>
      <div className="field"><label>{t('Only for space (key, optional)')}</label><input className="input" value={form.spaceKey} onChange={(e) => setForm({ ...form, spaceKey: e.target.value.toUpperCase() })} /></div>
      {error && <p className="error-text">{error}</p>}
    </Modal>
  )
}
