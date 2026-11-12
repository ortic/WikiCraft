import { t } from '../i18n'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Copy, Download, Fingerprint, LogOut, Monitor, ShieldCheck, Smartphone } from 'lucide-react'
import { ApiError, api, formatDate } from '../api'
import { cancelled, createPasskey, passkeysSupported } from '../passkeys'
import { Modal, Spinner, useToast } from '../ui'

interface MfaStatus {
  enabled: boolean
  enabledAt: string | null
  recoveryCodesLeft: number
  hasPassword: boolean
  passkeys: number
}

interface Passkey {
  id: string
  name: string
  /** synced to the person's other devices */
  backedUp: boolean
  createdAt: string
  lastUsedAt: string | null
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
  if (!ua) return t('Unknown device')
  const browser = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : /curl|reqwest|python|Go-http/i.test(ua) ? t('API client') : t('Browser')
  const os = /Windows/.test(ua) ? 'Windows' : /Mac OS X|Macintosh/.test(ua) ? 'macOS' : /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iOS' : /Linux/.test(ua) ? 'Linux' : ''
  return os ? `${browser} on ${os}` : browser
}

function RecoveryCodes({ codes, onClose }: { codes: string[]; onClose: () => void }) {
  const text = codes.join('\n')
  return (
    <Modal title={t('Save your recovery codes')} onClose={onClose} footer={<button className="btn btn-primary" onClick={onClose}>{t('I\'ve saved them')}</button>}>
      <p style={{ marginTop: 0 }}>{t('Each code signs you in once if you lose your phone. Store them somewhere safe — they aren\'t shown again.')}</p>
      <div className="recovery-codes">{codes.map((c) => <code key={c}>{c}</code>)}</div>
      <div className="row" style={{ marginTop: 12 }}>
        <button className="btn" onClick={() => navigator.clipboard?.writeText(text)}><Copy size={14} />{' '}{t('Copy')}</button>
        <a className="btn" href={`data:text/plain;charset=utf-8,${encodeURIComponent('' + t('WikiCraft recovery codes') + '\n\n' + text + '\n')}`} download="wikicraft-recovery-codes.txt"><Download size={14} />{' '}{t('Download')}</a>
      </div>
    </Modal>
  )
}

/**
 * Runs a change to the sign-in methods. Signed in a while ago, the server wants the password
 * first: asks for it and tries again. Undefined when the person cancels.
 */
async function withPassword<T>(call: (password?: string) => Promise<T>, hasPassword: boolean, question: string, noPassword: string): Promise<T | undefined> {
  try {
    return await call()
  } catch (e) {
    if (!(e instanceof ApiError && e.status === 403)) throw e
    if (!hasPassword) throw new Error(noPassword)
    const password = window.prompt(question)
    if (!password) return undefined
    return await call(password)
  }
}

type Setup = { secret: string; uri: string; qrSvg: string }

function EnableDialog({ setup, onDone, onClose }: { setup: Setup; onDone: (codes: string[]) => void; onClose: () => void }) {
  const [code, setCode] = useState('')
  const [error, setError] = useState<string | null>(null)
  const confirm = async () => {
    setError(null)
    try {
      const r = await api.post<{ recoveryCodes: string[] }>('/me/mfa/enable', { code })
      onDone(r.recoveryCodes)
    } catch (e) {
      setError(e instanceof Error ? e.message : t('Failed'))
    }
  }
  return (
    <Modal title={t('Set up two-factor authentication')} onClose={onClose} footer={<><button className="btn" onClick={onClose}>{t('Cancel')}</button><button className="btn btn-primary" disabled={code.replace(/\D/g, '').length !== 6} onClick={confirm}>{t('Turn on')}</button></>}>
      <div className="mfa-setup">
        <div className="mfa-qr" dangerouslySetInnerHTML={{ __html: setup.qrSvg }} />
        <div>
          <ol className="small" style={{ paddingLeft: 18, marginTop: 0 }}>
            <li>{t('Scan the code with an authenticator app (1Password, Google Authenticator, Microsoft Authenticator, Authy, …).')}</li>
            <li>{t('Or enter this key by hand:')}{' '}<code style={{ wordBreak: 'break-all' }}>{setup.secret.replace(/(.{4})/g, '$1 ').trim()}</code></li>
            <li>{t('Type the 6-digit code the app shows:')}</li>
          </ol>
          <input className="input otp-input" inputMode="numeric" autoComplete="one-time-code" value={code} onChange={(e) => setCode(e.target.value)} placeholder="123 456" autoFocus onKeyDown={(e) => e.key === 'Enter' && confirm()} />
          {error && <p className="error-text">{error}</p>}
        </div>
      </div>
    </Modal>
  )
}

/** Passkeys: sign in with the device's fingerprint, face or PIN, or use one as the second step. */
function Passkeys({ hasPassword }: { hasPassword: boolean }) {
  const qc = useQueryClient()
  const toast = useToast()
  const list = useQuery({ queryKey: ['passkeys'], queryFn: () => api.get<Passkey[]>('/me/passkeys') })
  const [busy, setBusy] = useState(false)
  const changed = () => {
    qc.invalidateQueries({ queryKey: ['passkeys'] })
    qc.invalidateQueries({ queryKey: ['mfa'] })
    // a passkey also fulfils the workspace's two-factor requirement
    qc.invalidateQueries({ queryKey: ['me'] })
  }
  const add = async () => {
    setBusy(true)
    try {
      type Options = { token: string; publicKey: Record<string, unknown> }
      const options = await withPassword(
        (password) => api.post<Options>('/me/passkeys/options', password ? { password } : {}),
        hasPassword,
        t('Enter your password to add a passkey'),
        t('Sign out and in again, then add the passkey'),
      )
      if (!options) return
      const credential = await createPasskey(options.publicKey)
      await api.post('/me/passkeys', { token: options.token, name: device(navigator.userAgent), credential })
      toast(t('Passkey added'))
      changed()
    } catch (e) {
      if (!cancelled(e)) toast(e instanceof Error ? e.message : t('Failed'))
    } finally {
      setBusy(false)
    }
  }
  const rename = async (p: Passkey) => {
    const name = window.prompt(t('Name of this passkey'), p.name)
    if (!name?.trim()) return
    await api.patch(`/me/passkeys/${p.id}`, { name })
    changed()
  }
  const remove = async (p: Passkey) => {
    if (!window.confirm(t('Remove the passkey “{name}”? You can’t sign in with it any more.', { name: p.name }))) return
    try {
      const done = await withPassword(
        (password) => api.del<true>(`/me/passkeys/${p.id}`, password ? { password } : undefined).then(() => true),
        hasPassword,
        t('Enter your password to remove the passkey'),
        t('Sign out and in again, then remove the passkey'),
      )
      if (!done) return
      toast(t('Passkey removed'))
      changed()
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Failed'))
    }
  }
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Fingerprint size={17} />{' '}{t('Passkeys')}</h3>
      <p className="muted" style={{ marginTop: 0 }}>{t('Sign in with your fingerprint, face or device PIN instead of a password, or use a passkey as the second step after it. Passkeys can’t be phished: they only work on this wiki.')}</p>
      {(list.data?.length ?? 0) > 0 && (
        <table className="list" style={{ marginBottom: 12 }}>
          <tbody>
            {list.data!.map((p) => (
              <tr key={p.id}>
                <td>
                  <b>{p.name}</b>
                  {p.backedUp && <span className="badge" style={{ marginInlineStart: 6 }} title={t('Synced to your other devices')}>{t('Synced')}</span>}
                </td>
                <td className="faint small">
                  {t('Added {date}', { date: formatDate(p.createdAt) })}
                  {p.lastUsedAt ? ` · ${t('last used {date}', { date: formatDate(p.lastUsedAt) })}` : ''}
                </td>
                <td style={{ textAlign: 'right', whiteSpace: 'nowrap' }}>
                  <button className="btn btn-ghost btn-sm" onClick={() => rename(p)}>{t('Rename')}</button>
                  <button className="btn btn-ghost btn-sm" onClick={() => remove(p)}>{t('Remove')}</button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {passkeysSupported() ? (
        <button className={`btn${list.data?.length ? '' : ' btn-primary'}`} disabled={busy} onClick={add}><Fingerprint size={15} />{' '}{t('Add a passkey')}</button>
      ) : (
        <p className="faint small" style={{ marginBottom: 0 }}>{t('This browser can’t use passkeys here (they need a secure https connection).')}</p>
      )}
    </div>
  )
}

/** Settings → Security: two-factor authentication and active sessions. */
export default function SecuritySettings() {
  const qc = useQueryClient()
  const toast = useToast()
  const mfa = useQuery({ queryKey: ['mfa'], queryFn: () => api.get<MfaStatus>('/me/mfa') })
  const sessions = useQuery({ queryKey: ['sessions'], queryFn: () => api.get<Session[]>('/me/sessions') })
  const [enabling, setEnabling] = useState<Setup | null>(null)
  const [codes, setCodes] = useState<string[] | null>(null)
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['mfa'] })
    qc.invalidateQueries({ queryKey: ['sessions'] })
  }
  const disable = async () => {
    const m = mfa.data!
    const secret = m.hasPassword ? window.prompt(t('Enter your password to turn off two-factor authentication')) : window.prompt(t('Enter a code from your authenticator app'))
    if (!secret) return
    try {
      await api.post('/me/mfa/disable', m.hasPassword ? { password: secret } : { code: secret })
      toast(t('Two-factor authentication is off'))
      refresh()
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Failed'))
    }
  }
  const startSetup = async () => {
    try {
      // with a passkey a second factor is on already: adding the app needs a recent sign-in
      const setup = await withPassword(
        (password) => api.post<Setup>('/me/mfa/setup', password ? { password } : {}),
        mfa.data?.hasPassword ?? true,
        t('Enter your password to set up two-factor authentication'),
        t('Sign out and in again, then set up two-factor authentication'),
      )
      if (setup) setEnabling(setup)
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Failed'))
    }
  }
  const regenerate = async () => {
    const code = window.prompt(t('Enter a code from your authenticator app to create new recovery codes'))
    if (!code) return
    try {
      const r = await api.post<{ recoveryCodes: string[] }>('/me/mfa/recovery-codes', { code })
      setCodes(r.recoveryCodes)
      refresh()
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Failed'))
    }
  }
  return (
    <>
      <Passkeys hasPassword={mfa.data?.hasPassword ?? true} />
      <div className="card" style={{ marginBottom: 16 }}>
        <h3 style={{ marginTop: 0 }} className="row"><ShieldCheck size={17} />{' '}{t('Two-factor authentication')}</h3>
        {!mfa.data ? (
          <Spinner />
        ) : mfa.data.enabled ? (
          <>
            <p className="muted" style={{ marginTop: 0 }}>{t('On since {date}. Signing in with your password also asks for a code from your authenticator app. {recoveryCodesLeft} recovery codes left.', { date: formatDate(mfa.data.enabledAt), recoveryCodesLeft: mfa.data.recoveryCodesLeft })}</p>
            <div className="row">
              <button className="btn" onClick={regenerate}>{t('New recovery codes')}</button>
              <button className="btn btn-ghost" onClick={disable}>{t('Turn off')}</button>
            </div>
          </>
        ) : (
          <>
            <p className="muted" style={{ marginTop: 0 }}>{t('Protect your account with a one-time code from an authenticator app in addition to your password.')}</p>
            <button className="btn btn-primary" onClick={startSetup}><Smartphone size={15} />{' '}{t('Set up two-factor authentication')}</button>
          </>
        )}
      </div>
      <div className="card" style={{ marginBottom: 16 }}>
        <div className="row">
          <h3 style={{ margin: 0 }} className="row"><Monitor size={17} />{' '}{t('Where you\'re signed in')}</h3>
          <span className="spacer" />
          {(sessions.data?.length ?? 0) > 1 && (
            <button className="btn small" onClick={async () => { const r = await api.post<{ revoked: number }>('/me/sessions/revoke-others'); toast(t('Signed out of {revoked} other sessions', { revoked: r.revoked })); refresh() }}>
              <LogOut size={14} />{' '}{t('Sign out everywhere else')}</button>
          )}
        </div>
        <table className="list" style={{ marginTop: 10 }}>
          <tbody>
            {sessions.data?.map((s) => (
              <tr key={s.id}>
                <td>
                  <b>{device(s.userAgent)}</b> {s.current && <span className="badge badge-accent">{t('this device')}</span>}
                  <div className="faint small">{s.ip ? `${s.ip} · ` : ''}{t('signed in {date} · active {date2}', { date: formatDate(s.createdAt), date2: formatDate(s.lastSeenAt) })}</div>
                </td>
                <td style={{ textAlign: 'right' }}>
                  {!s.current && <button className="btn small" onClick={async () => { await api.del(`/me/sessions/${s.id}`); refresh() }}>{t('Sign out')}</button>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {enabling && <EnableDialog setup={enabling} onClose={() => setEnabling(null)} onDone={(c) => { setEnabling(null); setCodes(c); refresh() }} />}
      {codes && <RecoveryCodes codes={codes} onClose={() => setCodes(null)} />}
    </>
  )
}
