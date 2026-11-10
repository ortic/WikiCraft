import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { KeyRound, Trash2, Copy, Bot, Mail } from 'lucide-react'
import { api, formatDate } from '../api'
import SecuritySettings from '../components/SecuritySettings'
import { ProfileFields } from './People'
import { useMe, useToast } from '../ui'

interface Token {
  id: string
  name: string
  createdAt: string
  expiresAt: string | null
  lastUsedAt: string | null
  scope?: 'read' | 'write'
}

/** Light, dark or high-contrast theme for this browser (System follows the device). */
function Appearance() {
  const [theme, setTheme] = useState(() => {
    try {
      return localStorage.getItem('wc-theme') ?? ''
    } catch {
      return ''
    }
  })
  const choose = (t: string) => {
    setTheme(t)
    if (t) document.documentElement.dataset.theme = t
    else delete document.documentElement.dataset.theme
    try {
      if (t) localStorage.setItem('wc-theme', t)
      else localStorage.removeItem('wc-theme')
    } catch {
      /* private mode */
    }
  }
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <h3 style={{ marginTop: 0 }}>Appearance</h3>
      <div className="field" style={{ marginBottom: 0 }}>
        <label htmlFor="theme">Theme (this browser)</label>
        <select id="theme" className="select" style={{ maxWidth: 320 }} value={theme} onChange={(e) => choose(e.target.value)}>
          <option value="">System (light or dark, more contrast if the device asks for it)</option>
          <option value="light">Light</option>
          <option value="dark">Dark</option>
          <option value="contrast">High contrast</option>
        </select>
      </div>
    </div>
  )
}

export default function Settings() {
  const me = useMe()
  const qc = useQueryClient()
  const toast = useToast()
  const user = me.data?.user
  const [name, setName] = useState(user?.name ?? '')
  const [password, setPassword] = useState('')
  const [tokenName, setTokenName] = useState('')
  const [tokenScope, setTokenScope] = useState<'read' | 'write'>('write')
  const [newToken, setNewToken] = useState<string | null>(null)
  const tokens = useQuery({ queryKey: ['tokens'], queryFn: () => api.get<Token[]>('/me/tokens') })
  if (!user) return null
  const mcpUrl = `${location.origin}/api/mcp`
  const claudeConfig = JSON.stringify({ mcpServers: { wikicraft: { type: 'http', url: mcpUrl, headers: { Authorization: `Bearer ${newToken ?? '<your token>'}` } } } }, null, 2)
  const copy = async (s: string) => {
    try {
      await navigator.clipboard.writeText(s)
      toast('Copied to clipboard')
    } catch {
      toast('Copy failed — select and copy manually')
    }
  }
  return (
    <div className="content-pad" style={{ maxWidth: 820 }}>
      <h1 style={{ marginTop: 0 }}>Your settings</h1>

      <div className="card" style={{ marginBottom: 16 }}>
        <h3 style={{ marginTop: 0 }}>Profile</h3>
        <div className="field"><label htmlFor="profile-name">Name</label><input id="profile-name" className="input" value={name} onChange={(e) => setName(e.target.value)} /></div>
        <div className="field"><label htmlFor="profile-email">E-mail</label><input id="profile-email" className="input" value={user.email ?? ''} disabled /></div>
        <div className="field"><label htmlFor="profile-password">New password (optional)</label><input id="profile-password" className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="at least 8 characters" /></div>
        <button
          className="btn btn-primary"
          onClick={async () => {
            try {
              await api.patch(`/users/${user.id}`, { name, ...(password ? { password } : {}) })
              setPassword('')
              qc.invalidateQueries({ queryKey: ['me'] })
              toast('Saved')
            } catch (e) {
              toast(e instanceof Error ? e.message : 'Failed')
            }
          }}
        >
          Save
        </button>
      </div>

      <ProfileFields userId={user.id} />

      <Appearance />

      <EmailPreferences />

      <SecuritySettings />

      <div className="card" style={{ marginBottom: 16 }}>
        <h3 style={{ marginTop: 0 }} className="row"><KeyRound size={17} /> Personal API tokens</h3>
        <p className="muted small" style={{ marginTop: 0 }}>
          Tokens act with your permissions. Use them for scripts (<code>Authorization: Bearer …</code>) and AI assistants; a read-only token can search and read
          but never change anything.
        </p>
        <div className="row" style={{ marginBottom: 12, flexWrap: 'wrap' }}>
          <input className="input" style={{ maxWidth: 300 }} placeholder="Token name, e.g. Claude Desktop" aria-label="Token name" value={tokenName} onChange={(e) => setTokenName(e.target.value)} />
          <select className="select" style={{ width: 'auto' }} value={tokenScope} onChange={(e) => setTokenScope(e.target.value as 'read' | 'write')} aria-label="What the token may do">
            <option value="write">Read and write</option>
            <option value="read">Read only</option>
          </select>
          <button
            className="btn"
            disabled={!tokenName.trim()}
            onClick={async () => {
              const r = await api.post<{ token: string }>('/me/tokens', { name: tokenName.trim(), scope: tokenScope })
              setNewToken(r.token)
              setTokenName('')
              qc.invalidateQueries({ queryKey: ['tokens'] })
            }}
          >
            Create token
          </button>
        </div>
        {newToken && (
          <div className="callout" data-callout="success" style={{ marginBottom: 12 }}>
            <div className="callout-body">
              <b>Copy your token now — it won't be shown again.</b>
              <div className="row" style={{ marginTop: 6 }}>
                <code style={{ wordBreak: 'break-all' }}>{newToken}</code>
                <button className="icon-btn" onClick={() => copy(newToken)}><Copy size={14} /></button>
              </div>
            </div>
          </div>
        )}
        <table className="list">
          <tbody>
            {tokens.data?.map((t) => (
              <tr key={t.id}>
                <td><b>{t.name}</b> {t.scope === 'read' && <span className="badge">read only</span>}</td>
                <td className="faint small">created {formatDate(t.createdAt)}</td>
                <td className="faint small">{t.lastUsedAt ? `used ${formatDate(t.lastUsedAt)}` : 'never used'}</td>
                <td style={{ width: 40 }}>
                  <button className="icon-btn" title="Revoke" onClick={async () => { await api.del(`/me/tokens/${t.id}`); qc.invalidateQueries({ queryKey: ['tokens'] }) }}>
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {tokens.data?.length === 0 && <div className="faint small">No tokens yet.</div>}
      </div>

      <div className="card">
        <h3 style={{ marginTop: 0 }} className="row"><Bot size={17} /> Connect an AI assistant (MCP)</h3>
        <p className="muted small" style={{ marginTop: 0 }}>
          WikiCraft has a built-in <a href="https://modelcontextprotocol.io" target="_blank" rel="noreferrer">Model Context Protocol</a> server. Assistants like Claude can search, read and write pages — with exactly your permissions.
          Endpoint: <code>{mcpUrl}</code>
        </p>
        <div className="row" style={{ marginBottom: 6 }}>
          <b className="small">Client configuration</b>
          <span className="spacer" />
          <button className="btn btn-sm" onClick={() => copy(claudeConfig)}><Copy size={13} /> Copy</button>
        </div>
        <pre style={{ background: 'var(--bg-soft)', padding: 12, borderRadius: 8, fontSize: 12.5, overflowX: 'auto' }}>{claudeConfig}</pre>
        <p className="faint small">Claude Code: <code>claude mcp add --transport http wikicraft {mcpUrl} --header "Authorization: Bearer &lt;token&gt;"</code></p>
        <p className="faint small">Tools: search, get_page, list_spaces, list_children, recent_changes, create_page, update_page, append_to_page, add_comment.</p>
      </div>
    </div>
  )
}

function EmailPreferences() {
  const qc = useQueryClient()
  const toast = useToast()
  const KIND_LABELS: Record<string, string> = {
    comment: 'Comments on pages I watch',
    reply: 'Replies to my comments',
    mention: 'Mentions in comments',
    page_mention: 'Mentions on pages',
    page_created: 'New pages in spaces I watch',
    task_due: 'My tasks on their due date',
    page_updated: 'Updates to pages and spaces I watch',
    like: 'Likes',
    share: 'Pages shared with me',
    verification_request: 'Requests to review a page',
    access_request: 'Requests for access to my pages',
  }
  const prefs = useQuery({ queryKey: ['preferences'], queryFn: () => api.get<{ emailNotifications: 'all' | 'important' | 'none'; emailDigest: 'soon' | 'hourly' | 'daily'; mailEnabled: boolean; muted: string[]; kinds: string[] }>('/me/preferences') })
  const p = prefs.data
  if (!p) return null
  const set = async (v: string) => {
    try {
      await api.put('/me/preferences', { emailNotifications: v })
      qc.invalidateQueries({ queryKey: ['preferences'] })
      toast('Saved')
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Failed')
    }
  }
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Mail size={17} /> E-mail notifications</h3>
      <p className="muted small" style={{ marginTop: 0 }}>
        Notifications you haven't seen in WikiCraft are e-mailed to you as one summary: a few minutes after, at most once an hour, or once a day.
        {!p.mailEnabled && ' (E-mail is not configured on this server yet.)'}
      </p>
      <div className="row" style={{ flexWrap: 'wrap', gap: 8 }}>
        <select className="select" style={{ maxWidth: 420 }} value={p.emailNotifications} onChange={(e) => set(e.target.value)} aria-label="E-mail notifications">
          <option value="important">Mentions, comments and replies (recommended)</option>
          <option value="all">Everything, including updates to pages I watch</option>
          <option value="none">Don't e-mail me</option>
        </select>
        {p.emailNotifications !== 'none' && (
          <select
            className="select"
            style={{ maxWidth: 260 }}
            value={p.emailDigest ?? 'soon'}
            aria-label="How often"
            onChange={async (e) => {
              await api.put('/me/preferences', { emailDigest: e.target.value })
              qc.invalidateQueries({ queryKey: ['preferences'] })
            }}
          >
            <option value="soon">A few minutes after</option>
            <option value="hourly">At most once an hour</option>
            <option value="daily">Once a day</option>
          </select>
        )}
      </div>
      <h4 style={{ margin: '18px 0 6px' }}>Notify me about</h4>
      <p className="muted small" style={{ marginTop: 0 }}>Turned-off events don't appear in WikiCraft and aren't e-mailed.</p>
      <div className="notify-kinds">
        {p.kinds.map((k) => (
          <label key={k} className="row small">
            <input
              type="checkbox"
              checked={!p.muted.includes(k)}
              onChange={async (e) => {
                const muted = e.target.checked ? p.muted.filter((m) => m !== k) : [...p.muted, k]
                await api.put('/me/preferences', { muted })
                qc.invalidateQueries({ queryKey: ['preferences'] })
                qc.invalidateQueries({ queryKey: ['notifications'] })
              }}
            />
            {KIND_LABELS[k] ?? k}
          </label>
        ))}
      </div>
    </div>
  )
}
