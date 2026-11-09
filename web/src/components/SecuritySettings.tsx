import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, Download, LogOut, Monitor, ShieldCheck, Smartphone } from 'lucide-react'
import { api, formatDate } from '../api'
import { Modal, Spinner, useToast } from '../ui'

interface MfaStatus {
  enabled: boolean
  enabledAt: string | null
  recoveryCodesLeft: number
  hasPassword: boolean
}

interface Session {
  id: string
  createdAt: string
  lastSeenAt: string
  userAgent: string | null
  ip: string | null
  current: boolean
}

function device(ua: string | null) {
  if (!ua) return 'Unknown device'
  const browser = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : /curl|reqwest|python|Go-http/i.test(ua) ? 'API client' : 'Browser'
  const os = /Windows/.test(ua) ? 'Windows' : /Mac OS X|Macintosh/.test(ua) ? 'macOS' : /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iOS' : /Linux/.test(ua) ? 'Linux' : ''
  return os ? `${browser} on ${os}` : browser
}

function RecoveryCodes({ codes, onClose }: { codes: string[]; onClose: () => void }) {
  const text = codes.join('\n')
  return (
    <Modal title="Save your recovery codes" onClose={onClose} footer={<button className="btn btn-primary" onClick={onClose}>I've saved them</button>}>
      <p style={{ marginTop: 0 }}>Each code signs you in once if you lose your phone. Store them somewhere safe — they aren't shown again.</p>
      <div className="recovery-codes">{codes.map((c) => <code key={c}>{c}</code>)}</div>
      <div className="row" style={{ marginTop: 12 }}>
        <button className="btn" onClick={() => navigator.clipboard?.writeText(text)}><Copy size={14} /> Copy</button>
        <a className="btn" href={`data:text/plain;charset=utf-8,${encodeURIComponent('WikiCraft recovery codes\n\n' + text + '\n')}`} download="wikicraft-recovery-codes.txt"><Download size={14} /> Download</a>
      </div>
    </Modal>
  )
}

function EnableDialog({ onDone, onClose }: { onDone: (codes: string[]) => void; onClose: () => void }) {
  const setup = useQuery({ queryKey: ['mfa-setup'], queryFn: () => api.post<{ secret: string; uri: string; qrSvg: string }>('/me/mfa/setup'), staleTime: Infinity, gcTime: 0 })
  const [code, setCode] = useState('')
  const [error, setError] = useState<string | null>(null)
  const confirm = async () => {
    setError(null)
    try {
      const r = await api.post<{ recoveryCodes: string[] }>('/me/mfa/enable', { code })
      onDone(r.recoveryCodes)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed')
    }
  }
  return (
    <Modal title="Set up two-factor authentication" onClose={onClose} footer={<><button className="btn" onClick={onClose}>Cancel</button><button className="btn btn-primary" disabled={code.replace(/\D/g, '').length !== 6} onClick={confirm}>Turn on</button></>}>
      {!setup.data ? (
        <Spinner />
      ) : (
        <div className="mfa-setup">
          <div className="mfa-qr" dangerouslySetInnerHTML={{ __html: setup.data.qrSvg }} />
          <div>
            <ol className="small" style={{ paddingLeft: 18, marginTop: 0 }}>
              <li>Scan the code with an authenticator app (1Password, Google Authenticator, Microsoft Authenticator, Authy, …).</li>
              <li>Or enter this key by hand: <code style={{ wordBreak: 'break-all' }}>{setup.data.secret.replace(/(.{4})/g, '$1 ').trim()}</code></li>
              <li>Type the 6-digit code the app shows:</li>
            </ol>
            <input className="input otp-input" inputMode="numeric" autoComplete="one-time-code" value={code} onChange={(e) => setCode(e.target.value)} placeholder="123 456" autoFocus onKeyDown={(e) => e.key === 'Enter' && confirm()} />
            {error && <p className="error-text">{error}</p>}
          </div>
        </div>
      )}
    </Modal>
  )
}

/** Settings → Security: two-factor authentication and active sessions. */
export default function SecuritySettings() {
  const qc = useQueryClient()
  const toast = useToast()
  const mfa = useQuery({ queryKey: ['mfa'], queryFn: () => api.get<MfaStatus>('/me/mfa') })
  const sessions = useQuery({ queryKey: ['sessions'], queryFn: () => api.get<Session[]>('/me/sessions') })
  const [enabling, setEnabling] = useState(false)
  const [codes, setCodes] = useState<string[] | null>(null)
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['mfa'] })
    qc.invalidateQueries({ queryKey: ['sessions'] })
  }
  const disable = async () => {
    const m = mfa.data!
    const secret = m.hasPassword ? window.prompt('Enter your password to turn off two-factor authentication') : window.prompt('Enter a code from your authenticator app')
    if (!secret) return
    try {
      await api.post('/me/mfa/disable', m.hasPassword ? { password: secret } : { code: secret })
      toast('Two-factor authentication is off')
      refresh()
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Failed')
    }
  }
  const regenerate = async () => {
    const code = window.prompt('Enter a code from your authenticator app to create new recovery codes')
    if (!code) return
    try {
      const r = await api.post<{ recoveryCodes: string[] }>('/me/mfa/recovery-codes', { code })
      setCodes(r.recoveryCodes)
      refresh()
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Failed')
    }
  }
  return (
    <>
      <div className="card" style={{ marginBottom: 16 }}>
        <h3 style={{ marginTop: 0 }} className="row"><ShieldCheck size={17} /> Two-factor authentication</h3>
        {!mfa.data ? (
          <Spinner />
        ) : mfa.data.enabled ? (
          <>
            <p className="muted" style={{ marginTop: 0 }}>
              On since {formatDate(mfa.data.enabledAt)}. Signing in with your password also asks for a code from your authenticator app. {mfa.data.recoveryCodesLeft} recovery codes left.
            </p>
            <div className="row">
              <button className="btn" onClick={regenerate}>New recovery codes</button>
              <button className="btn btn-ghost" onClick={disable}>Turn off</button>
            </div>
          </>
        ) : (
          <>
            <p className="muted" style={{ marginTop: 0 }}>Protect your account with a one-time code from an authenticator app in addition to your password.</p>
            <button className="btn btn-primary" onClick={() => setEnabling(true)}><Smartphone size={15} /> Set up two-factor authentication</button>
          </>
        )}
      </div>
      <div className="card" style={{ marginBottom: 16 }}>
        <div className="row">
          <h3 style={{ margin: 0 }} className="row"><Monitor size={17} /> Where you're signed in</h3>
          <span className="spacer" />
          {(sessions.data?.length ?? 0) > 1 && (
            <button className="btn small" onClick={async () => { const r = await api.post<{ revoked: number }>('/me/sessions/revoke-others'); toast(`Signed out of ${r.revoked} other sessions`); refresh() }}>
              <LogOut size={14} /> Sign out everywhere else
            </button>
          )}
        </div>
        <table className="list" style={{ marginTop: 10 }}>
          <tbody>
            {sessions.data?.map((s) => (
              <tr key={s.id}>
                <td>
                  <b>{device(s.userAgent)}</b> {s.current && <span className="badge badge-accent">this device</span>}
                  <div className="faint small">{s.ip ? `${s.ip} · ` : ''}signed in {formatDate(s.createdAt)} · active {formatDate(s.lastSeenAt)}</div>
                </td>
                <td style={{ textAlign: 'right' }}>
                  {!s.current && <button className="btn small" onClick={async () => { await api.del(`/me/sessions/${s.id}`); refresh() }}>Sign out</button>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {enabling && <EnableDialog onClose={() => setEnabling(false)} onDone={(c) => { setEnabling(false); setCodes(c); refresh() }} />}
      {codes && <RecoveryCodes codes={codes} onClose={() => setCodes(null)} />}
    </>
  )
}
