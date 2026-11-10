import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, Globe, Link2, Trash2 } from 'lucide-react'
import { api, formatDate } from '../api'
import { Modal, Spinner, useToast } from '../ui'

interface ShareLink {
  id: string
  token: string
  includeChildren: boolean
  createdByName: string | null
  createdAt: string
  expiresAt: string | null
  views: number
  lastViewedAt: string | null
  hasPassword?: boolean
}

/** Copy the page link for colleagues, or create a public read-only link for anyone. */
export default function ShareDialog({ pageId, title, restricted, onClose }: { pageId: string; title: string; restricted: boolean; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const links = useQuery({ queryKey: ['share-links', pageId], queryFn: () => api.get<{ enabled: boolean; links: ShareLink[] }>(`/pages/${pageId}/share-links`) })
  const [children, setChildren] = useState(true)
  const [expiry, setExpiry] = useState('')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const url = (token: string) => `${location.origin}/s/${token}`
  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text)
      toast('Link copied')
    } catch {
      toast('Copy failed — select the link and copy it')
    }
  }
  const create = async () => {
    setBusy(true)
    try {
      const r = await api.post<{ token: string }>(`/pages/${pageId}/share-links`, { includeChildren: children, expiresInDays: expiry ? Number(expiry) : undefined, password: password || undefined })
      await qc.invalidateQueries({ queryKey: ['share-links', pageId] })
      // the password goes to the visitors separately; the field doesn't keep it
      setPassword('')
      copy(url(r.token))
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not create the link')
    } finally {
      setBusy(false)
    }
  }
  const revoke = async (id: string) => {
    await api.del(`/share-links/${id}`)
    qc.invalidateQueries({ queryKey: ['share-links', pageId] })
    toast('Link turned off')
  }
  const internal = `${location.origin}/pages/${pageId}`
  return (
    <Modal title={`Share “${title}”`} onClose={onClose} width={620}>
      <h4 className="row" style={{ margin: '0 0 6px' }}><Link2 size={15} /> People in your workspace</h4>
      <div className="row">
        <input className="input" readOnly value={internal} onFocus={(e) => e.target.select()} />
        <button className="btn" onClick={() => copy(internal)}><Copy size={14} /> Copy</button>
      </div>
      <p className="faint small" style={{ margin: '6px 0 18px' }}>They see the page if their space role and the page restrictions allow it.</p>

      <h4 className="row" style={{ margin: '0 0 6px' }}><Globe size={15} /> Anyone with the link</h4>
      {links.isLoading ? (
        <Spinner />
      ) : !links.data?.enabled ? (
        <p className="muted small">Public links are turned off for this workspace (Admin → Overview).</p>
      ) : restricted ? (
        <p className="muted small">This page has view restrictions, so it can't be shared publicly.</p>
      ) : (
        <>
          {links.data.links.map((l) => (
            <div key={l.id} className="share-link">
              <input className="input" readOnly value={url(l.token)} onFocus={(e) => e.target.select()} />
              <button className="btn" onClick={() => copy(url(l.token))}><Copy size={14} /></button>
              <button className="btn btn-ghost" title="Turn off this link" onClick={() => revoke(l.id)}><Trash2 size={14} /></button>
              <div className="faint small" style={{ gridColumn: '1 / -1' }}>
                {l.includeChildren ? 'Page and sub-pages' : 'This page only'} · {l.expiresAt ? `expires ${formatDate(l.expiresAt)}` : 'no expiry'}{l.hasPassword ? ' · password' : ''} · {l.views} views
                {l.createdByName ? ` · created by ${l.createdByName}` : ''}
              </div>
            </div>
          ))}
          <div className="row" style={{ flexWrap: 'wrap', marginTop: 8 }}>
            <label className="row small"><input type="checkbox" checked={children} onChange={(e) => setChildren(e.target.checked)} /> Include sub-pages</label>
            <select className="select" style={{ width: 160 }} value={expiry} onChange={(e) => setExpiry(e.target.value)}>
              <option value="">No expiry</option>
              <option value="1">Expires in 1 day</option>
              <option value="7">Expires in 7 days</option>
              <option value="30">Expires in 30 days</option>
              <option value="90">Expires in 90 days</option>
            </select>
            <input
              className="input"
              style={{ width: 200 }}
              type="password"
              autoComplete="new-password"
              placeholder="Password (optional)"
              aria-label="Password for the link (optional)"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            <span className="spacer" />
            <button className="btn btn-primary" disabled={busy} onClick={create}>{busy ? <Spinner /> : <Globe size={14} />} Create public link</button>
          </div>
          <p className="faint small" style={{ marginBottom: 0 }}>Anyone with a public link can read the page without signing in. Sub-pages with their own restrictions are never included.</p>
        </>
      )}
    </Modal>
  )
}
