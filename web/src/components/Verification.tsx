// Page verification: the badge under the title, the "verify" dialog for editors and the
// "request review" dialog for everyone.

import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { BadgeCheck, BadgeAlert, CheckCircle2, Lock } from 'lucide-react'
import { api, formatDate } from '../api'
import { Modal, Spinner, useToast } from '../ui'

export interface Verification {
  status: 'none' | 'verified' | 'expired'
  verifiedBy?: { id: string; name: string } | null
  verifiedAt?: string
  expiresAt?: string | null
  note?: string | null
  editedSince?: boolean
}

export function useVerification(pageId: string) {
  return useQuery({ queryKey: ['verification', pageId], queryFn: () => api.get<Verification>(`/pages/${pageId}/verification`), staleTime: 30_000 })
}

export function VerificationBadge({ pageId, onClick }: { pageId: string; onClick?: () => void }) {
  const q = useVerification(pageId)
  const v = q.data
  if (!v || v.status === 'none') return null
  const by = v.verifiedBy?.name ?? 'someone'
  if (v.status === 'expired') {
    return (
      <button type="button" className="badge badge-warning verify-badge" onClick={onClick} title={`Verified by ${by} · ${formatDate(v.verifiedAt)} · expired ${formatDate(v.expiresAt)}`}>
        <BadgeAlert size={13} /> Verification expired
      </button>
    )
  }
  const until = v.expiresAt ? ` · valid until ${formatDate(v.expiresAt)}` : ''
  const tip = `Verified by ${by} · ${formatDate(v.verifiedAt)}${until}${v.note ? ` · ${v.note}` : ''}${v.editedSince ? ' · edited since' : ''}`
  return (
    <button type="button" className={`badge verify-badge ${v.editedSince ? 'badge-warning' : 'badge-success'}`} onClick={onClick} title={tip}>
      <BadgeCheck size={13} /> Verified{v.editedSince ? ', edited since' : ''}
    </button>
  )
}

const DURATIONS: [string, number | null][] = [
  ['30 days', 30],
  ['90 days', 90],
  ['6 months', 182],
  ['1 year', 365],
  ['No expiry', null],
]

export function VerifyDialog({ pageId, onClose }: { pageId: string; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const current = useVerification(pageId).data
  const [days, setDays] = useState<number | null>(90)
  const [note, setNote] = useState('')
  const [busy, setBusy] = useState(false)
  const done = () => {
    qc.invalidateQueries({ queryKey: ['verification', pageId] })
    onClose()
  }
  const verify = async () => {
    setBusy(true)
    try {
      await api.put(`/pages/${pageId}/verification`, { days, note: note || undefined })
      toast('Page verified')
      done()
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not verify')
    } finally {
      setBusy(false)
    }
  }
  const remove = async () => {
    await api.del(`/pages/${pageId}/verification`)
    toast('Verification removed')
    done()
  }
  return (
    <Modal
      title="Verify this page"
      onClose={onClose}
      width={460}
      footer={
        <>
          {current && current.status !== 'none' && (
            <button className="btn btn-danger" onClick={remove}>Remove verification</button>
          )}
          <span className="spacer" />
          <button className="btn" onClick={onClose}>Cancel</button>
          <button className="btn btn-primary" disabled={busy} onClick={verify}>{busy ? <Spinner /> : <BadgeCheck size={15} />} Verify</button>
        </>
      }
    >
      <p className="muted" style={{ marginTop: 0 }}>
        Confirm that the content is accurate. Readers see a verified badge; when it expires, the page is flagged for review.
      </p>
      {current && current.status !== 'none' && (
        <p className="small faint">
          Currently {current.status === 'expired' ? 'expired' : 'verified'} by {current.verifiedBy?.name ?? 'someone'} ({formatDate(current.verifiedAt)})
          {current.expiresAt ? ` (until ${formatDate(current.expiresAt)})` : ''}.
        </p>
      )}
      <div className="field">
        <label>Valid for</label>
        <div className="row" style={{ flexWrap: 'wrap', gap: 6 }}>
          {DURATIONS.map(([label, d]) => (
            <button key={label} type="button" className={`btn btn-sm ${days === d ? 'btn-primary' : ''}`} onClick={() => setDays(d)}>{label}</button>
          ))}
        </div>
      </div>
      <div className="field">
        <label>Note (optional)</label>
        <input className="input" value={note} maxLength={500} placeholder="e.g. Checked against the 2027 policy" onChange={(e) => setNote(e.target.value)} />
      </div>
    </Modal>
  )
}

export function RequestReviewDialog({ pageId, onClose }: { pageId: string; onClose: () => void }) {
  const toast = useToast()
  const [query, setQuery] = useState('')
  const [who, setWho] = useState<{ id: string; name: string } | null>(null)
  const [note, setNote] = useState('')
  const users = useQuery({
    queryKey: ['user-search', query],
    queryFn: () => api.get<{ id: string; name: string; email?: string }[]>(`/users?q=${encodeURIComponent(query)}`),
    enabled: query.trim().length > 0 && !who,
  })
  const send = async () => {
    if (!who) return
    try {
      await api.post(`/pages/${pageId}/verification/request`, { userId: who.id, note: note || undefined })
      toast(`Asked ${who.name} to review this page`)
      onClose()
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not send the request')
    }
  }
  return (
    <Modal
      title="Request a review"
      onClose={onClose}
      width={460}
      footer={
        <>
          <span className="spacer" />
          <button className="btn" onClick={onClose}>Cancel</button>
          <button className="btn btn-primary" disabled={!who} onClick={send}>Send request</button>
        </>
      }
    >
      <p className="muted" style={{ marginTop: 0 }}>Ask someone who can edit this page to check it and verify it.</p>
      <div className="field">
        <label>Reviewer</label>
        {who ? (
          <div className="row">
            <span className="badge badge-accent">{who.name}</span>
            <button className="btn btn-sm" onClick={() => setWho(null)}>Change</button>
          </div>
        ) : (
          <>
            <input className="input" autoFocus placeholder="Search people" value={query} onChange={(e) => setQuery(e.target.value)} />
            {(users.data ?? []).slice(0, 6).map((u) => (
              <div key={u.id} className="menu-item" onClick={() => setWho(u)}>{u.name}</div>
            ))}
          </>
        )}
      </div>
      <div className="field">
        <label>Message (optional)</label>
        <input className="input" value={note} maxLength={500} placeholder="What should be checked?" onChange={(e) => setNote(e.target.value)} />
      </div>
    </Modal>
  )
}

/** Shown instead of a page the reader can't open: maybe it doesn't exist, maybe it's restricted. */
export function NoAccess({ pageId, signedIn }: { pageId: string; signedIn: boolean }) {
  const toast = useToast()
  const [message, setMessage] = useState('')
  const [sent, setSent] = useState(false)
  const send = async () => {
    try {
      await api.post(`/pages/${pageId}/access-request`, { message: message || undefined })
      setSent(true)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not send the request')
    }
  }
  return (
    <div className="card no-access">
      <Lock size={28} className="faint" />
      <h2>This page isn’t available to you</h2>
      <p className="muted">It may have been deleted or moved, or it is restricted to certain people.</p>
      {signedIn &&
        (sent ? (
          <p><CheckCircle2 size={15} color="var(--success)" style={{ verticalAlign: -2 }} /> Request sent. The page’s admins will let you know.</p>
        ) : (
          <div className="no-access-form">
            <input className="input" placeholder="Why do you need access? (optional)" value={message} maxLength={500} onChange={(e) => setMessage(e.target.value)} />
            <button className="btn btn-primary" onClick={send}>Request access</button>
          </div>
        ))}
    </div>
  )
}
