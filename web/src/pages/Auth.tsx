import { useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Building2, KeyRound } from 'lucide-react'
import { api } from '../api'
import { Logo } from '../ui'

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

export function Login() {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(() => new URLSearchParams(location.search).get('error'))
  const info = useQuery({ queryKey: ['auth-providers'], queryFn: () => api.get<{ passwordLogin: boolean; providers: Provider[] }>('/auth/providers') })
  const next = new URLSearchParams(location.search).get('next') ?? '/'
  const [busy, setBusy] = useState(false)
  const redirects = (info.data?.providers ?? []).filter((p) => p.loginUrl)
  const directory = (info.data?.providers ?? []).filter((p) => p.type === 'ldap')
  // the form signs in local accounts and directory (LDAP / AD) accounts
  const showForm = info.data ? info.data.passwordLogin || directory.length > 0 : true
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await api.post('/auth/login', { email, password })
      location.href = next.startsWith('/') && !next.startsWith('//') ? next : '/'
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Login failed')
      setBusy(false)
    }
  }
  const wide = { width: '100%', justifyContent: 'center', height: 38 } as const
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
              <label>Password</label>
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
      </form>
    </div>
  )
}
