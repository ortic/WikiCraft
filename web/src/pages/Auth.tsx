import { t } from '../i18n'
import { useEffect, useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Building2, Fingerprint, KeyRound } from 'lucide-react'
import { api } from '../api'
import { cancelled, passkeysSupported, signWithPasskey } from '../passkeys'
import { Logo, Spinner } from '../ui'

export function Setup() {
  const qc = useQueryClient()
  const [form, setForm] = useState({ workspaceName: '', name: '', email: '', password: '' })
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const set = (k: keyof typeof form) => (e: React.ChangeEvent<HTMLInputElement>) => setForm({ ...form, [k]: e.target.value })
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await api.post('/setup', form)
      await qc.invalidateQueries()
      location.href = '/'
    } catch (err) {
      setError(err instanceof Error ? err.message : t('Setup failed'))
      setBusy(false)
    }
  }
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>{t('Welcome to WikiCraft')}</h1>
        <p className="muted" style={{ marginTop: 0 }}>{t('Create your workspace and the first admin account.')}</p>
        <div className="field">
          <label htmlFor="setup-workspace">{t('Workspace name')}</label>
          <input id="setup-workspace" className="input" required value={form.workspaceName} onChange={set('workspaceName')} placeholder={t('Acme Inc.')} autoFocus />
        </div>
        <div className="field">
          <label htmlFor="setup-name">{t('Your name')}</label>
          <input id="setup-name" className="input" required value={form.name} onChange={set('name')} />
        </div>
        <div className="field">
          <label htmlFor="setup-email">{t('E-mail')}</label>
          <input id="setup-email" className="input" type="email" required value={form.email} onChange={set('email')} />
        </div>
        <div className="field">
          <label htmlFor="setup-password">{t('Password')}</label>
          <input id="setup-password" className="input" type="password" required minLength={8} value={form.password} onChange={set('password')} />
        </div>
        {error && <p className="error-text">{error}</p>}
        <button className="btn btn-primary" style={{ width: '100%', justifyContent: 'center', height: 38 }} disabled={busy}>{t('Create workspace')}</button>
      </form>
    </div>
  )
}

type Provider = { id: string; name: string; type: 'oidc' | 'github' | 'ldap' | 'saml'; preset?: string | null; loginUrl?: string | null }

function ProviderIcon({ p }: { p: Provider }) {
  if (p.preset === 'microsoft')
    return (
      <svg width="15" height="15" viewBox="0 0 21 21" aria-hidden="true">
        <rect x="1" y="1" width="9" height="9" fill="#f25022" />
        <rect x="11" y="1" width="9" height="9" fill="#7fba00" />
        <rect x="1" y="11" width="9" height="9" fill="#00a4ef" />
        <rect x="11" y="11" width="9" height="9" fill="#ffb900" />
      </svg>
    )
  if (p.type === 'ldap') return <Building2 size={15} />
  return <KeyRound size={15} />
}

/** Only same-site paths: no scheme-relative `//`, no whitespace/control characters or
 * backslashes (browsers drop tabs, so `/\t/evil` would become `//evil`). */
export function safeNext(n: string | null | undefined): string {
  return n && /^\/(?![\/\\])[^\s\\\u0000-\u001f\u007f]*$/.test(n) ? n : '/'
}

export function Login() {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(() => new URLSearchParams(location.search).get('error'))
  const info = useQuery({ queryKey: ['auth-providers'], queryFn: () => api.get<{ passwordLogin: boolean; passwordReset: boolean; providers: Provider[] }>('/auth/providers') })
  const signup = useQuery({ queryKey: ['signup-info'], queryFn: () => api.get<{ enabled: boolean; domains: string[] }>('/auth/signup') })
  const next = new URLSearchParams(location.search).get('next') ?? '/'
  const [busy, setBusy] = useState(false)
  // `?mfa=1`: an SSO login or password reset left a challenge cookie for the code step
  const [mfaToken, setMfaToken] = useState<string | null>(() => (new URLSearchParams(location.search).get('mfa') ? '' : null))
  const [code, setCode] = useState('')
  // second step: what the person can use (a code and/or a passkey); unknown after SSO until asked
  const [methods, setMethods] = useState<string[] | null>(null)
  useEffect(() => {
    if (mfaToken !== null && methods === null)
      api
        .post<{ methods: string[] }>('/auth/mfa/passkey', { mfaToken: mfaToken || undefined })
        .then((r) => setMethods(r.methods))
        .catch(() => setMethods(['code']))
  }, [mfaToken, methods])
  const redirects = (info.data?.providers ?? []).filter((p) => p.loginUrl)
  const directory = (info.data?.providers ?? []).filter((p) => p.type === 'ldap')
  // the form signs in local accounts and directory (LDAP / AD) accounts
  const showForm = info.data ? info.data.passwordLogin || directory.length > 0 : true
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      if (mfaToken !== null) {
        const r = await api.post<{ next?: string | null }>('/auth/mfa', { mfaToken: mfaToken || undefined, code })
        location.href = safeNext(r.next ?? next)
        return
      } else {
        const r = await api.post<{ ok?: boolean; mfaRequired?: boolean; mfaToken?: string; methods?: string[] }>('/auth/login', { email, password })
        if (r.mfaRequired && r.mfaToken) {
          setMethods(r.methods ?? null)
          setMfaToken(r.mfaToken)
          setBusy(false)
          return
        }
      }
      location.href = safeNext(next)
    } catch (err) {
      setError(err instanceof Error ? err.message : t('Login failed'))
      setBusy(false)
    }
  }
  // the second step with a passkey
  const passkeyStep = async () => {
    setBusy(true)
    setError(null)
    try {
      const step = await api.post<{ publicKey: Record<string, unknown> | null }>('/auth/mfa/passkey', { mfaToken: mfaToken || undefined })
      if (!step.publicKey) throw new Error(t('You have no passkey'))
      const passkey = await signWithPasskey(step.publicKey)
      const r = await api.post<{ next?: string | null }>('/auth/mfa', { mfaToken: mfaToken || undefined, passkey })
      location.href = safeNext(r.next ?? next)
    } catch (e) {
      if (!cancelled(e)) setError(e instanceof Error ? e.message : t('Login failed'))
      setBusy(false)
    }
  }
  // signing in with a passkey alone (it verified the person: two factors in one)
  const passkeyLogin = async () => {
    setBusy(true)
    setError(null)
    try {
      const o = await api.post<{ token: string; publicKey: Record<string, unknown> }>('/auth/passkey/options')
      const credential = await signWithPasskey(o.publicKey)
      await api.post('/auth/passkey', { token: o.token, credential })
      location.href = safeNext(next)
    } catch (e) {
      if (!cancelled(e)) setError(e instanceof Error ? e.message : t('Login failed'))
      setBusy(false)
    }
  }
  const wide = { width: '100%', justifyContent: 'center', height: 38 } as const
  const withCode = methods === null || methods.includes('code')
  const withPasskey = !!methods?.includes('passkey') && passkeysSupported()
  if (mfaToken !== null)
    return (
      <div className="auth">
        <form className="auth-card" onSubmit={submit}>
          <Logo />
          <h1>{t('Two-factor sign-in')}</h1>
          {withPasskey && (
            <>
              <p className="muted" style={{ marginTop: 0 }}>{t('Confirm with your passkey (fingerprint, face, device PIN or security key).')}</p>
              <button type="button" className="btn btn-primary" style={wide} disabled={busy} onClick={passkeyStep} autoFocus={!withCode}><Fingerprint size={16} />{' '}{t('Use your passkey')}</button>
              {withCode && <div className="faint small" style={{ textAlign: 'center', margin: '14px 0' }}>{t('or with a code')}</div>}
            </>
          )}
          {withCode && (
            <>
              {!withPasskey && <p className="muted" style={{ marginTop: 0 }}>{t('Enter the 6-digit code from your authenticator app, or one of your recovery codes.')}</p>}
              <div className="field">
                <label htmlFor="login-code">{t('Code')}</label>
                <input id="login-code" className="input otp-input" inputMode="numeric" autoComplete="one-time-code" required value={code} onChange={(e) => setCode(e.target.value)} autoFocus={!withPasskey} placeholder="123 456" />
              </div>
            </>
          )}
          {error && <p className="error-text">{error}</p>}
          {withCode && <button className={`btn${withPasskey ? '' : ' btn-primary'}`} style={wide} disabled={busy}>{t('Verify')}</button>}
          <p className="small" style={{ textAlign: 'center', marginBottom: 0 }}>
            <a href="/login" onClick={(e) => { e.preventDefault(); setMfaToken(null); setMethods(null); setCode(''); setError(null) }}>{t('Use a different account')}</a>
          </p>
        </form>
      </div>
    )
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>{t('Log in')}</h1>
        <p className="muted" style={{ marginTop: 0 }}>{t('Welcome back.')}</p>
        {redirects.length > 0 && (
          <>
            <div className="sso-buttons">
              {redirects.map((p) => (
                <a key={p.id} className="btn" style={wide} href={`${p.loginUrl}?next=${encodeURIComponent(next)}`}>
                  <ProviderIcon p={p} />{' '}{t('Continue with {name}', { name: p.name })}
                </a>
              ))}
            </div>
            {showForm && <div className="faint small" style={{ textAlign: 'center', margin: '14px 0' }}>{directory.length > 0 ? (directory.length === 1 ? t('or with your {name} account', { name: directory[0].name }) : t('or with your directory account')) : t('or with your password')}</div>}
          </>
        )}
        {showForm && (
          <>
            <div className="field">
              <label htmlFor="login-email">{directory.length > 0 ? t('E-mail or username') : t('E-mail')}</label>
              <input id="login-email" className="input" type={directory.length > 0 ? 'text' : 'email'} autoComplete="username" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
            </div>
            <div className="field">
              <div className="row">
                <label htmlFor="login-password">{t('Password')}</label>
                {info.data?.passwordReset && (
                  <a className="small" style={{ marginLeft: 'auto', fontWeight: 400 }} href="/forgot-password">{t('Forgot password?')}</a>
                )}
              </div>
              <input id="login-password" className="input" type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
            </div>
          </>
        )}
        {error && <p className="error-text">{error}</p>}
        {showForm && (
          <button className="btn btn-primary" style={wide} disabled={busy}>{t('Log in')}</button>
        )}
        {passkeysSupported() && (
          <button type="button" className="btn" style={{ ...wide, marginTop: 10 }} disabled={busy} onClick={passkeyLogin}><Fingerprint size={16} />{' '}{t('Sign in with a passkey')}</button>
        )}
        {signup.data?.enabled && (
          <p className="small" style={{ textAlign: 'center', marginBottom: 0 }}>{t('New here?')}{' '}<a href="/signup">{t('Create an account')}</a>
          </p>
        )}
      </form>
    </div>
  )
}

const wideBtn = { width: '100%', justifyContent: 'center', height: 38 } as const

function tokenParam() {
  return new URLSearchParams(location.search).get('token') ?? ''
}

/** `/signup` — people with an allowed e-mail domain ask for an account. */
export function Signup() {
  const info = useQuery({ queryKey: ['signup-info'], queryFn: () => api.get<{ enabled: boolean; domains: string[] }>('/auth/signup') })
  const [email, setEmail] = useState('')
  const [sent, setSent] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await api.post('/auth/signup', { email })
      setSent(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : t('Something went wrong'))
    } finally {
      setBusy(false)
    }
  }
  const domains = info.data?.domains ?? []
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>{t('Create an account')}</h1>
        {info.data && !info.data.enabled ? (
          <p className="muted">{t('Accounts are by invitation. Ask an admin of this wiki to invite you.')}</p>
        ) : sent ? (
          <p className="muted">{t('We sent a confirmation link to')}{' '}<b>{email}</b>{t('. Open it to choose your name and password. It\'s valid for 48 hours.')}</p>
        ) : (
          <>
            <p className="muted" style={{ marginTop: 0 }}>{t('Use your work e-mail')}{domains.length ? ` (${domains.map((d) => '@' + d).join(', ')})` : ''}{t('. We\'ll send you a link to confirm it.')}</p>
            <div className="field">
              <label htmlFor="auth-email">{t('E-mail')}</label>
              <input id="auth-email" className="input" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
            </div>
            {error && <p className="error-text">{error}</p>}
            <button className="btn btn-primary" style={wideBtn} disabled={busy}>{t('Send confirmation link')}</button>
          </>
        )}
        <p className="small" style={{ textAlign: 'center', marginBottom: 0 }}><a href="/login">{t('Back to log in')}</a></p>
      </form>
    </div>
  )
}

/** `/forgot-password` — asks for a reset link. */
export function ForgotPassword() {
  const [email, setEmail] = useState('')
  const [sent, setSent] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await api.post('/auth/forgot', { email })
      setSent(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : t('Something went wrong'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>{t('Reset your password')}</h1>
        {sent ? (
          <p className="muted">{t('If an account with a password exists for')}{' '}<b>{email}</b>{t(', we\'ve sent a link to choose a new one. It\'s valid for one hour.')}</p>
        ) : (
          <>
            <p className="muted" style={{ marginTop: 0 }}>{t('Enter your e-mail and we\'ll send you a link to choose a new password.')}</p>
            <div className="field">
              <label htmlFor="auth-email">{t('E-mail')}</label>
              <input id="auth-email" className="input" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
            </div>
            {error && <p className="error-text">{error}</p>}
            <button className="btn btn-primary" style={wideBtn} disabled={busy}>{t('Send reset link')}</button>
          </>
        )}
        <p className="small" style={{ textAlign: 'center', marginBottom: 0 }}><a href="/login">{t('Back to log in')}</a></p>
      </form>
    </div>
  )
}

/** `/reset-password?token=…` */
export function ResetPassword() {
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    if (password !== confirm) return setError(t('The passwords don\'t match'))
    setBusy(true)
    setError(null)
    try {
      const r = await api.post<{ mfaRequired?: boolean }>('/auth/reset', { token: tokenParam(), password })
      location.href = r.mfaRequired ? '/login?mfa=1' : '/'
    } catch (err) {
      setError(err instanceof Error ? err.message : t('Something went wrong'))
      setBusy(false)
    }
  }
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>{t('Choose a new password')}</h1>
        <p className="muted" style={{ marginTop: 0 }}>{t('You\'ll be signed out everywhere else.')}</p>
        <div className="field">
          <label htmlFor="reset-password">{t('New password')}</label>
          <input id="reset-password" className="input" type="password" autoComplete="new-password" minLength={8} required value={password} onChange={(e) => setPassword(e.target.value)} autoFocus />
        </div>
        <div className="field">
          <label htmlFor="reset-confirm">{t('Repeat password')}</label>
          <input id="reset-confirm" className="input" type="password" autoComplete="new-password" minLength={8} required value={confirm} onChange={(e) => setConfirm(e.target.value)} />
        </div>
        {error && <p className="error-text">{error}</p>}
        <button className="btn btn-primary" style={wideBtn} disabled={busy}>{t('Save and sign in')}</button>
      </form>
    </div>
  )
}

/** `/accept-invite?token=…` */
export function AcceptInvite() {
  const info = useQuery({ queryKey: ['invite', tokenParam()], queryFn: () => api.get<{ email: string; name: string; workspace: string; passwordLogin: boolean; signup?: boolean }>(`/auth/invite?token=${encodeURIComponent(tokenParam())}`), retry: false })
  const sso = useQuery({ queryKey: ['auth-providers'], queryFn: () => api.get<{ providers: Provider[] }>('/auth/providers') })
  const [name, setName] = useState<string | null>(null)
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await api.post('/auth/accept-invite', { token: tokenParam(), name: name ?? info.data?.name ?? '', password })
      location.href = '/'
    } catch (err) {
      setError(err instanceof Error ? err.message : t('Something went wrong'))
      setBusy(false)
    }
  }
  const redirects = (sso.data?.providers ?? []).filter((p) => p.loginUrl)
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        {info.error ? (
          <>
            <h1>{t('Invitation not valid')}</h1>
            <p className="muted">{info.error instanceof Error ? info.error.message : t('This link is invalid or has expired.')}{' '}{t('Ask an administrator for a new invitation.')}</p>
            <p className="small"><a href="/login">{t('Go to log in')}</a></p>
          </>
        ) : !info.data ? (
          <Spinner />
        ) : (
          <>
            <h1>{t('Join {workspace}', { workspace: info.data.workspace })}</h1>
            <p className="muted" style={{ marginTop: 0 }}>{info.data.signup ? '' + t('Your address is confirmed:') + ' ' : '' + t('You were invited as') + ' '}<b>{info.data.email}</b>.</p>
            {redirects.length > 0 && (
              <div className="sso-buttons" style={{ marginBottom: 14 }}>
                {redirects.map((p) => (
                  <a key={p.id} className="btn" style={wideBtn} href={`${p.loginUrl}?next=%2F`}><ProviderIcon p={p} />{' '}{t('Continue with {name}', { name: p.name })}</a>
                ))}
              </div>
            )}
            {info.data.passwordLogin && (
              <>
                <div className="field">
                  <label htmlFor="invite-name">{t('Your name')}</label>
                  <input id="invite-name" className="input" required value={name ?? info.data.name} onChange={(e) => setName(e.target.value)} autoFocus />
                </div>
                <div className="field">
                  <label htmlFor="invite-password">{t('Choose a password')}</label>
                  <input id="invite-password" className="input" type="password" autoComplete="new-password" minLength={8} required value={password} onChange={(e) => setPassword(e.target.value)} />
                </div>
                {error && <p className="error-text">{error}</p>}
                <button className="btn btn-primary" style={wideBtn} disabled={busy}>{t('Create my account')}</button>
              </>
            )}
          </>
        )}
      </form>
    </div>
  )
}
