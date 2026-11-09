import { useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { KeyRound } from 'lucide-react'
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

export function Login() {
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [error, setError] = useState<string | null>(() => new URLSearchParams(location.search).get('error'))
  const sso = useQuery({ queryKey: ['oidc'], queryFn: () => api.get<{ enabled: boolean; name: string; passwordLogin: boolean }>('/auth/oidc') })
  const next = new URLSearchParams(location.search).get('next') ?? '/'
  const [busy, setBusy] = useState(false)
  const submit = async (e: FormEvent) => {
    e.preventDefault()
    setBusy(true)
    setError(null)
    try {
      await api.post('/auth/login', { email, password })
      const next = new URLSearchParams(location.search).get('next')
      location.href = next && next.startsWith('/') && !next.startsWith('//') ? next : '/'
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Login failed')
      setBusy(false)
    }
  }
  return (
    <div className="auth">
      <form className="auth-card" onSubmit={submit}>
        <Logo />
        <h1>Log in</h1>
        <p className="muted" style={{ marginTop: 0 }}>Welcome back.</p>
        {sso.data?.enabled && (
          <>
            <a className="btn" style={{ width: '100%', justifyContent: 'center', height: 38 }} href={`/api/auth/oidc/login?next=${encodeURIComponent(next)}`}>
              <KeyRound size={15} /> Continue with {sso.data.name}
            </a>
            {sso.data.passwordLogin && <div className="faint small" style={{ textAlign: 'center', margin: '14px 0' }}>or with your password</div>}
          </>
        )}
        {sso.data?.passwordLogin !== false && (
          <>
            <div className="field">
              <label>E-mail</label>
              <input className="input" type="email" required value={email} onChange={(e) => setEmail(e.target.value)} autoFocus />
            </div>
            <div className="field">
              <label>Password</label>
              <input className="input" type="password" required value={password} onChange={(e) => setPassword(e.target.value)} />
            </div>
          </>
        )}
        {error && <p className="error-text">{error}</p>}
        {sso.data?.passwordLogin !== false && (
          <button className="btn btn-primary" style={{ width: '100%', justifyContent: 'center', height: 38 }} disabled={busy}>
            Log in
          </button>
        )}
      </form>
    </div>
  )
}
