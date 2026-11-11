import { AVAILABLE, chooseLanguage, rememberLanguage, t as tr } from '../i18n'
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

/** Light, dark or high-contrast theme for this browser (System follows the device), and the
 *  interface language (profile). */
function Appearance() {
  const me = useMe()
  const toast = useToast()
  const workspaceLanguage = me.data?.workspace?.defaultLanguage
  const fallback = AVAILABLE.find((l) => l.code === chooseLanguage(null, workspaceLanguage))?.name ?? 'English'
  const setLanguage = async (code: string) => {
    try {
      await api.put('/me/profile', { language: code })
      rememberLanguage(chooseLanguage(code, workspaceLanguage))
      window.location.reload()
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Failed'))
    }
  }
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
      <h3 style={{ marginTop: 0 }}>{tr('Appearance')}</h3>
      {me.data?.user && (
        <div className="field">
          <label htmlFor="language">{tr('Language')}</label>
          <select id="language" className="select" style={{ maxWidth: 320 }} value={me.data.language ?? ''} onChange={(e) => setLanguage(e.target.value)}>
            <option value="">{workspaceLanguage ? tr('Workspace default ({language})', { language: fallback }) : tr('Browser language ({language})', { language: fallback })}</option>
            {AVAILABLE.map((l) => (
              <option key={l.code} value={l.code} lang={l.code}>{l.name}</option>
            ))}
          </select>
        </div>
      )}
      <div className="field" style={{ marginBottom: 0 }}>
        <label htmlFor="theme">{tr('Theme (this browser)')}</label>
        <select id="theme" className="select" style={{ maxWidth: 320 }} value={theme} onChange={(e) => choose(e.target.value)}>
          <option value="">{tr('System (light or dark, more contrast if the device asks for it)')}</option>
          <option value="light">{tr('Light')}</option>
          <option value="dark">{tr('Dark')}</option>
          <option value="contrast">{tr('High contrast')}</option>
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
      toast(tr('Copied to clipboard'))
    } catch {
      toast(tr('Copy failed — select and copy manually'))
    }
  }
  return (
    <div className="content-pad" style={{ maxWidth: 820 }}>
      <h1 style={{ marginTop: 0 }}>{tr('Your settings')}</h1>

      <div className="card" style={{ marginBottom: 16 }}>
        <h3 style={{ marginTop: 0 }}>{tr('Profile')}</h3>
        <div className="field"><label htmlFor="profile-name">{tr('Name')}</label><input id="profile-name" className="input" value={name} onChange={(e) => setName(e.target.value)} /></div>
        <div className="field"><label htmlFor="profile-email">{tr('E-mail')}</label><input id="profile-email" className="input" value={user.email ?? ''} disabled /></div>
        <div className="field"><label htmlFor="profile-password">{tr('New password (optional)')}</label><input id="profile-password" className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder={tr('at least 8 characters')} /></div>
        <button
          className="btn btn-primary"
          onClick={async () => {
            try {
              await api.patch(`/users/${user.id}`, { name, ...(password ? { password } : {}) })
              setPassword('')
              qc.invalidateQueries({ queryKey: ['me'] })
              toast(tr('Saved'))
            } catch (e) {
              toast(e instanceof Error ? e.message : tr('Failed'))
            }
          }}
        >{tr('Save')}</button>
      </div>

      <ProfileFields userId={user.id} />

      <Appearance />

      <EmailPreferences />

      <SecuritySettings />

      <div className="card" style={{ marginBottom: 16 }}>
        <h3 style={{ marginTop: 0 }} className="row"><KeyRound size={17} />{' '}{tr('Personal API tokens')}</h3>
        <p className="muted small" style={{ marginTop: 0 }}>{tr('Tokens act with your permissions. Use them for scripts (')}<code>Authorization: Bearer …</code>{tr(') and AI assistants; a read-only token can search and read but never change anything.')}</p>
        <div className="row" style={{ marginBottom: 12, flexWrap: 'wrap' }}>
          <input className="input" style={{ maxWidth: 300 }} placeholder={tr('Token name, e.g. Claude Desktop')} aria-label={tr('Token name')} value={tokenName} onChange={(e) => setTokenName(e.target.value)} />
          <select className="select" style={{ width: 'auto' }} value={tokenScope} onChange={(e) => setTokenScope(e.target.value as 'read' | 'write')} aria-label={tr('What the token may do')}>
            <option value="write">{tr('Read and write')}</option>
            <option value="read">{tr('Read only')}</option>
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
          >{tr('Create token')}</button>
        </div>
        {newToken && (
          <div className="callout" data-callout="success" style={{ marginBottom: 12 }}>
            <div className="callout-body">
              <b>{tr('Copy your token now — it won\'t be shown again.')}</b>
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
                <td><b>{t.name}</b> {t.scope === 'read' && <span className="badge">{tr('read only')}</span>}</td>
                <td className="faint small">{tr('created {date}', { date: formatDate(t.createdAt) })}</td>
                <td className="faint small">{t.lastUsedAt ? tr('used {date}', { date: formatDate(t.lastUsedAt) }) : tr('never used')}</td>
                <td style={{ width: 40 }}>
                  <button className="icon-btn" title={tr('Revoke')} onClick={async () => { await api.del(`/me/tokens/${t.id}`); qc.invalidateQueries({ queryKey: ['tokens'] }) }}>
                    <Trash2 size={14} />
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {tokens.data?.length === 0 && <div className="faint small">{tr('No tokens yet.')}</div>}
      </div>

      <div className="card">
        <h3 style={{ marginTop: 0 }} className="row"><Bot size={17} />{' '}{tr('Connect an AI assistant (MCP)')}</h3>
        <p className="muted small" style={{ marginTop: 0 }}>{tr('WikiCraft has a built-in')}{' '}<a href="https://modelcontextprotocol.io" target="_blank" rel="noreferrer">{tr('Model Context Protocol')}</a>{' '}{tr('server. Assistants like Claude can search, read and write pages — with exactly your permissions. Endpoint:')}{' '}<code>{mcpUrl}</code>
        </p>
        <div className="row" style={{ marginBottom: 6 }}>
          <b className="small">{tr('Client configuration')}</b>
          <span className="spacer" />
          <button className="btn btn-sm" onClick={() => copy(claudeConfig)}><Copy size={13} />{' '}{tr('Copy')}</button>
        </div>
        <pre style={{ background: 'var(--bg-soft)', padding: 12, borderRadius: 8, fontSize: 12.5, overflowX: 'auto' }}>{claudeConfig}</pre>
        <p className="faint small">{tr('Claude Code:')}{' '}<code>claude mcp add --transport http wikicraft {mcpUrl} --header "Authorization: Bearer &lt;token&gt;"</code></p>
        <p className="faint small">{tr('Tools: search, get_page, list_spaces, list_children, recent_changes, create_page, update_page, append_to_page, add_comment.')}</p>
      </div>
    </div>
  )
}

function EmailPreferences() {
  const qc = useQueryClient()
  const toast = useToast()
  const KIND_LABELS: Record<string, string> = {
    comment: tr('Comments on pages I watch'),
    reply: tr('Replies to my comments'),
    mention: tr('Mentions in comments'),
    page_mention: tr('Mentions on pages'),
    page_created: tr('New pages in spaces I watch'),
    task_due: tr('My tasks on their due date'),
    page_updated: tr('Updates to pages and spaces I watch'),
    like: tr('Likes'),
    share: tr('Pages shared with me'),
    verification_request: tr('Requests to review a page'),
    access_request: tr('Requests for access to my pages'),
  }
  const prefs = useQuery({ queryKey: ['preferences'], queryFn: () => api.get<{ emailNotifications: 'all' | 'important' | 'none'; emailDigest: 'soon' | 'hourly' | 'daily'; mailEnabled: boolean; muted: string[]; kinds: string[] }>('/me/preferences') })
  const p = prefs.data
  if (!p) return null
  const set = async (v: string) => {
    try {
      await api.put('/me/preferences', { emailNotifications: v })
      qc.invalidateQueries({ queryKey: ['preferences'] })
      toast(tr('Saved'))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Failed'))
    }
  }
  return (
    <div className="card" style={{ marginBottom: 16 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Mail size={17} />{' '}{tr('E-mail notifications')}</h3>
      <p className="muted small" style={{ marginTop: 0 }}>{tr('Notifications you haven\'t seen in WikiCraft are e-mailed to you as one summary: a few minutes after, at most once an hour, or once a day.')}{!p.mailEnabled && ' ' + tr('(E-mail is not configured on this server yet.)') + ''}
      </p>
      <div className="row" style={{ flexWrap: 'wrap', gap: 8 }}>
        <select className="select" style={{ maxWidth: 420 }} value={p.emailNotifications} onChange={(e) => set(e.target.value)} aria-label={tr('E-mail notifications')}>
          <option value="important">{tr('Mentions, comments and replies (recommended)')}</option>
          <option value="all">{tr('Everything, including updates to pages I watch')}</option>
          <option value="none">{tr('Don\'t e-mail me')}</option>
        </select>
        {p.emailNotifications !== 'none' && (
          <select
            className="select"
            style={{ maxWidth: 260 }}
            value={p.emailDigest ?? 'soon'}
            aria-label={tr('How often')}
            onChange={async (e) => {
              await api.put('/me/preferences', { emailDigest: e.target.value })
              qc.invalidateQueries({ queryKey: ['preferences'] })
            }}
          >
            <option value="soon">{tr('A few minutes after')}</option>
            <option value="hourly">{tr('At most once an hour')}</option>
            <option value="daily">{tr('Once a day')}</option>
          </select>
        )}
      </div>
      <h4 style={{ margin: '18px 0 6px' }}>{tr('Notify me about')}</h4>
      <p className="muted small" style={{ marginTop: 0 }}>{tr('Turned-off events don\'t appear in WikiCraft and aren\'t e-mailed.')}</p>
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
