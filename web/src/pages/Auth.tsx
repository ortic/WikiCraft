import { useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Building2, KeyRound } from 'lucide-react'
import { api } from '../api'
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
      setError(err instanceof Error ? err.message : 'Setup failed')
      setBusy(false)
    }
  }
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>Welcome to WikiCraft</h1>
        <p className="muted" style={{ marginTop: 0 }}>Create your workspace and the first admin account.</p>
        <div className="field">
          <label>Workspace name</label>
          <input className="input" required value={form.workspaceName} onChange={set('workspaceName')} placeholder="Acme Inc." autoFocus />
        </div>
        <div className="field">
          <label>Your name</label>
          <input className="input" required value={form.name} onChange={set('name')} />
        </div>
        <div className="field">
          <label>E-mail</label>
          <input className="input" type="email" required value={form.email} onChange={set('email')} />
        </div>
        <div className="field">
          <label>Password</label>
          <input className="input" type="password" required minLength={8} value={form.password} onChange={set('password')} />
        </div>
        {error && <p className="error-text">{error}</p>}
        <button className="btn btn-primary" style={{ width: '100%', justifyContent: 'center', height: 38 }} disabled={busy}>
          Create workspace
        </button>
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
        const r = await api.post<{ ok?: boolean; mfaRequired?: boolean; mfaToken?: string }>('/auth/login', { email, password })
        if (r.mfaRequired && r.mfaToken) {
          setMfaToken(r.mfaToken)
          setBusy(false)
          return
        }
      }
      location.href = safeNext(next)
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Login failed')
      setBusy(false)
    }
  }
  const wide = { width: '100%', justifyContent: 'center', height: 38 } as const
  if (mfaToken !== null)
    return (
      <div className="auth">
        <form className="auth-card" onSubmit={submit}>
          <Logo />
          <h1>Two-factor sign-in</h1>
          <p className="muted" style={{ marginTop: 0 }}>Enter the 6-digit code from your authenticator app, or one of your recovery codes.</p>
          <div className="field">
            <label>Code</label>
            <input className="input otp-input" inputMode="numeric" autoComplete="one-time-code" required value={code} onChange={(e) => setCode(e.target.value)} autoFocus placeholder="123 456" />
          </div>
          {error && <p className="error-text">{error}</p>}
          <button className="btn btn-primary" style={wide} disabled={busy}>Verify</button>
          <p className="small" style={{ textAlign: 'center', marginBottom: 0 }}>
            <a href="/login" onClick={(e) => { e.preventDefault(); setMfaToken(null); setCode(''); setError(null) }}>Use a different account</a>
          </p>
        </form>
      </div>
    )
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>Log in</h1>
        <p className="muted" style={{ marginTop: 0 }}>Welcome back.</p>
        {redirects.length > 0 && (
          <>
            <div className="sso-buttons">
              {redirects.map((p) => (
                <a key={p.id} className="btn" style={wide} href={`${p.loginUrl}?next=${encodeURIComponent(next)}`}>
                  <ProviderIcon p={p} /> Continue with {p.name}
                </a>
              ))}
            </div>
            {showForm && <div className="faint small" style={{ textAlign: 'center', margin: '14px 0' }}>{directory.length > 0 ? `or with your ${directory.length === 1 ? directory[0].name : 'directory'} account` : 'or with your password'}</div>}
          </>
        )}
        {showForm && (
          <>
            <div className="field">
              <label>{directory.length > 0 ? 'E-mail or username' : 'E-mail'}</label>
              <input className="input" type={directory.length > 0 ? 'text' : 'email'} autoComplete="username" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
            </div>
            <div className="field">
              <label className="row">
                Password
                {info.data?.passwordReset && (
                  <a className="small" style={{ marginLeft: 'auto', fontWeight: 400 }} href="/forgot-password">Forgot password?</a>
                )}
              </label>
              <input className="input" type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
            </div>
          </>
        )}
        {error && <p className="error-text">{error}</p>}
        {showForm && (
          <button className="btn btn-primary" style={wide} disabled={busy}>
            Log in
          </button>
        )}
        {signup.data?.enabled && (
          <p className="small" style={{ textAlign: 'center', marginBottom: 0 }}>
            New here? <a href="/signup">Create an account</a>
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
      setError(err instanceof Error ? err.message : 'Something went wrong')
    } finally {
      setBusy(false)
    }
  }
  const domains = info.data?.domains ?? []
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>Create an account</h1>
        {info.data && !info.data.enabled ? (
          <p className="muted">Accounts are by invitation. Ask an admin of this wiki to invite you.</p>
        ) : sent ? (
          <p className="muted">We sent a confirmation link to <b>{email}</b>. Open it to choose your name and password. It's valid for 48 hours.</p>
        ) : (
          <>
            <p className="muted" style={{ marginTop: 0 }}>
              Use your work e-mail{domains.length ? ` (${domains.map((d) => '@' + d).join(', ')})` : ''}. We'll send you a link to confirm it.
            </p>
            <div className="field">
              <label>E-mail</label>
              <input className="input" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
            </div>
            {error && <p className="error-text">{error}</p>}
            <button className="btn btn-primary" style={wideBtn} disabled={busy}>Send confirmation link</button>
          </>
        )}
        <p className="small" style={{ textAlign: 'center', marginBottom: 0 }}><a href="/login">Back to log in</a></p>
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
      setError(err instanceof Error ? err.message : 'Something went wrong')
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>Reset your password</h1>
        {sent ? (
          <p className="muted">If an account with a password exists for <b>{email}</b>, we've sent a link to choose a new one. It's valid for one hour.</p>
        ) : (
          <>
            <p className="muted" style={{ marginTop: 0 }}>Enter your e-mail and we'll send you a link to choose a new password.</p>
            <div className="field">
              <label>E-mail</label>
              <input className="input" type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
            </div>
            {error && <p className="error-text">{error}</p>}
            <button className="btn btn-primary" style={wideBtn} disabled={busy}>Send reset link</button>
          </>
        )}
        <p className="small" style={{ textAlign: 'center', marginBottom: 0 }}><a href="/login">Back to log in</a></p>
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
    if (password !== confirm) return setError("The passwords don't match")
    setBusy(true)
    setError(null)
    try {
      const r = await api.post<{ mfaRequired?: boolean }>('/auth/reset', { token: tokenParam(), password })
      location.href = r.mfaRequired ? '/login?mfa=1' : '/'
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Something went wrong')
      setBusy(false)
    }
  }
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>Choose a new password</h1>
        <p className="muted" style={{ marginTop: 0 }}>You'll be signed out everywhere else.</p>
        <div className="field">
          <label>New password</label>
          <input className="input" type="password" autoComplete="new-password" minLength={8} required value={password} onChange={(e) => setPassword(e.target.value)} autoFocus />
        </div>
        <div className="field">
          <label>Repeat password</label>
          <input className="input" type="password" autoComplete="new-password" minLength={8} required value={confirm} onChange={(e) => setConfirm(e.target.value)} />
        </div>
        {error && <p className="error-text">{error}</p>}
        <button className="btn btn-primary" style={wideBtn} disabled={busy}>Save and sign in</button>
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
      setError(err instanceof Error ? err.message : 'Something went wrong')
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
            <h1>Invitation not valid</h1>
            <p className="muted">{info.error instanceof Error ? info.error.message : 'This link is invalid or has expired.'} Ask an administrator for a new invitation.</p>
            <p className="small"><a href="/login">Go to log in</a></p>
          </>
        ) : !info.data ? (
          <Spinner />
        ) : (
          <>
            <h1>Join {info.data.workspace}</h1>
            <p className="muted" style={{ marginTop: 0 }}>{info.data.signup ? 'Your address is confirmed: ' : 'You were invited as '}<b>{info.data.email}</b>.</p>
            {redirects.length > 0 && (
              <div className="sso-buttons" style={{ marginBottom: 14 }}>
                {redirects.map((p) => (
                  <a key={p.id} className="btn" style={wideBtn} href={`${p.loginUrl}?next=%2F`}><ProviderIcon p={p} /> Continue with {p.name}</a>
                ))}
              </div>
            )}
            {info.data.passwordLogin && (
              <>
                <div className="field">
                  <label>Your name</label>
                  <input className="input" required value={name ?? info.data.name} onChange={(e) => setName(e.target.value)} autoFocus />
                </div>
                <div className="field">
                  <label>Choose a password</label>
                  <input className="input" type="password" autoComplete="new-password" minLength={8} required value={password} onChange={(e) => setPassword(e.target.value)} />
                </div>
                {error && <p className="error-text">{error}</p>}
                <button className="btn btn-primary" style={wideBtn} disabled={busy}>Create my account</button>
              </>
            )}
          </>
        )}
      </form>
    </div>
  )
}
