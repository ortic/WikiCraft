import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { KeyRound, Trash2, Copy, Bot, Mail } from 'lucide-react'
import { api, formatDate } from '../api'
import SecuritySettings from '../components/SecuritySettings'
import { useMe, useToast } from '../ui'

interface Token {
  id: string
  name: string
  createdAt: string
  expiresAt: string | null
  lastUsedAt: string | null
}

export default function Settings() {
  const me = useMe()
  const qc = useQueryClient()
  const toast = useToast()
  const user = me.data?.user
  const [name, setName] = useState(user?.name ?? '')
  const [password, setPassword] = useState('')
  const [tokenName, setTokenName] = useState('')
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
        <div className="field"><label>Name</label><input className="input" value={name} onChange={(e) => setName(e.target.value)} /></div>
        <div className="field"><label>E-mail</label><input className="input" value={user.email ?? ''} disabled /></div>
        <div className="field"><label>New password (optional)</label><input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="at least 8 characters" /></div>
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

      <EmailPreferences />

      <SecuritySettings />

      <div className="card" style={{ marginBottom: 16 }}>
        <h3 style={{ marginTop: 0 }} className="row"><KeyRound size={17} /> Personal API tokens</h3>
        <p className="muted small" style={{ marginTop: 0 }}>Tokens act with your permissions. Use them for scripts (<code>Authorization: Bearer …</code>) and AI assistants.</p>
        <div className="row" style={{ marginBottom: 12 }}>
          <input className="input" style={{ maxWidth: 300 }} placeholder="Token name, e.g. Claude Desktop" value={tokenName} onChange={(e) => setTokenName(e.target.value)} />
          <button
            className="btn"
            disabled={!tokenName.trim()}
            onClick={async () => {
              const r = await api.post<{ token: string }>('/me/tokens', { name: tokenName.trim() })
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
                <td><b>{t.name}</b></td>
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
    page_updated: 'Updates to pages I watch',
    like: 'Likes',
    share: 'Pages shared with me',
    verification_request: 'Requests to review a page',
    access_request: 'Requests for access to my pages',
  }
  const prefs = useQuery({ queryKey: ['preferences'], queryFn: () => api.get<{ emailNotifications: 'all' | 'important' | 'none'; mailEnabled: boolean; muted: string[]; kinds: string[] }>('/me/preferences') })
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
        Notifications you haven't seen in WikiCraft after a couple of minutes are e-mailed to you as one summary.
        {!p.mailEnabled && ' (E-mail is not configured on this server yet.)'}
      </p>
      <select className="select" style={{ maxWidth: 420 }} value={p.emailNotifications} onChange={(e) => set(e.target.value)}>
        <option value="important">Mentions, comments and replies (recommended)</option>
        <option value="all">Everything, including updates to pages I watch</option>
        <option value="none">Don't e-mail me</option>
      </select>
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
