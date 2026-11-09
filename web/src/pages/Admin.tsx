import { useEffect, useRef, useState } from 'react'
import { Link, NavLink, Navigate, Route, Routes } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Upload, FileSearch, CheckCircle2, XCircle, UserPlus, Users, Activity, Database, RefreshCw, Mail, Send, Globe } from 'lucide-react'
import { api, formatBytes, formatDate, type Job } from '../api'
import { Avatar, Modal, Spinner, useMe, useToast } from '../ui'
import AdminWebhooks from './AdminWebhooks'
import AdminSpaces from './AdminSpaces'

export default function Admin() {
  const me = useMe()
  if (me.data?.user && !['owner', 'admin'].includes(me.data.user.role)) return <div className="empty">Administrators only.</div>
  return (
    <div className="content-pad">
      <h1 style={{ marginTop: 0 }}>Administration</h1>
      <div className="tabs">
        <NavLink end to="/admin" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>Overview</NavLink>
        <NavLink to="/admin/spaces" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>Spaces</NavLink>
        <NavLink to="/admin/users" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>Users</NavLink>
        <NavLink to="/admin/groups" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>Groups</NavLink>
        <NavLink to="/admin/import" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>Import</NavLink>
        <NavLink to="/admin/webhooks" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>Webhooks</NavLink>
        <NavLink to="/admin/audit" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>Audit log</NavLink>
      </div>
      <Routes>
        <Route index element={<Overview />} />
        <Route path="spaces" element={<AdminSpaces />} />
        <Route path="users" element={<UsersAdmin />} />
        <Route path="groups" element={<GroupsAdmin />} />
        <Route path="import" element={<ImportAdmin />} />
        <Route path="webhooks" element={<AdminWebhooks />} />
        <Route path="audit" element={<Audit />} />
        <Route path="*" element={<Navigate to="/admin" replace />} />
      </Routes>
    </div>
  )
}

function Overview() {
  const toast = useToast()
  const stats = useQuery({ queryKey: ['admin-stats'], queryFn: () => api.get<Record<string, number>>('/admin/stats') })
  const s = stats.data
  const items: [string, string][] = s
    ? [
        ['Active users', String(s.users)],
        ['Imported authors (placeholders)', String(s.placeholderUsers)],
        ['Spaces', String(s.spaces)],
        ['Pages', String(s.pages)],
        ['Page versions', String(s.versions)],
        ['Comments', String(s.comments)],
        ['Attachments', String(s.attachments)],
        ['Storage', formatBytes(s.storageBytes)],
        ['Live editing sessions', String(s.liveRooms)],
      ]
    : []
  return (
    <div>
      {stats.isLoading && <Spinner />}
      <div className="grid-cards">
        {items.map(([k, v]) => (
          <div key={k} className="stat">
            <div className="v">{v}</div>
            <div className="k">{k}</div>
          </div>
        ))}
      </div>
      <div className="row" style={{ marginTop: 24 }}>
        <button
          className="btn"
          onClick={async () => {
            const r = await api.post<{ indexed: number }>('/admin/reindex')
            toast(`Re-indexing ${r.indexed} pages`)
          }}
        >
          <RefreshCw size={14} /> Rebuild search index
        </button>
        <Link className="btn" to="/admin/import">
          <Upload size={14} /> Import from Confluence
        </Link>
      </div>
      <WorkspaceSettings />
      <MailStatus />
    </div>
  )
}

function WorkspaceSettings() {
  const qc = useQueryClient()
  const toast = useToast()
  const settings = useQuery({ queryKey: ['admin-settings'], queryFn: () => api.get<{ publicLinks: boolean }>('/admin/settings') })
  if (!settings.data) return null
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Globe size={17} /> Sharing</h3>
      <label className="row">
        <input
          type="checkbox"
          checked={settings.data.publicLinks}
          onChange={async (e) => {
            await api.put('/admin/settings', { publicLinks: e.target.checked })
            qc.invalidateQueries({ queryKey: ['admin-settings'] })
            toast(e.target.checked ? 'Public links are allowed' : 'Public links are turned off — existing links stop working')
          }}
        />
        Allow public links (anyone with the link can read a shared page)
      </label>
      <p className="faint small" style={{ marginBottom: 0 }}>Turning this off disables every existing public link immediately; turning it back on restores them.</p>
    </div>
  )
}

function MailStatus() {
  const toast = useToast()
  const mail = useQuery({ queryKey: ['admin-mail'], queryFn: () => api.get<{ enabled: boolean; from: string; pending: number; failed: number; sentLastDay: number; lastError: string | null }>('/admin/mail') })
  const [busy, setBusy] = useState(false)
  const m = mail.data
  if (!m) return null
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Mail size={17} /> E-mail</h3>
      {m.enabled ? (
        <>
          <p className="muted" style={{ marginTop: 0 }}>
            Sending as <b>{m.from}</b> · {m.sentLastDay} sent in the last 24 h · {m.pending} queued{m.failed ? ` · ${m.failed} failed` : ''}
          </p>
          {m.lastError && <p className="error-text small">Last error: {m.lastError}</p>}
          <button className="btn" disabled={busy} onClick={async () => {
            setBusy(true)
            try {
              const r = await api.post<{ to: string }>('/admin/mail/test', {})
              toast(`Test e-mail sent to ${r.to}`)
            } catch (e) {
              toast(e instanceof Error ? e.message : 'Sending failed')
            } finally {
              setBusy(false)
            }
          }}>{busy ? <Spinner /> : <Send size={14} />} Send a test e-mail to me</button>
        </>
      ) : (
        <p className="muted" style={{ margin: 0 }}>
          Not configured. Set <code>WIKICRAFT_SMTP_URL</code> (and <code>WIKICRAFT_MAIL_FROM</code>) to send invitations, password-reset links and notification e-mails.
          Until then, invitation links are shown to you to share yourself.
        </p>
      )}
    </div>
  )
}

interface UserRow {
  id: string
  name: string
  email: string | null
  role: string
  status: string
  lastLoginAt: string | null
}

function UsersAdmin() {
  const qc = useQueryClient()
  const toast = useToast()
  const [showPlaceholders, setShowPlaceholders] = useState(false)
  const users = useQuery({ queryKey: ['admin-users', showPlaceholders], queryFn: () => api.get<UserRow[]>(`/users?include_placeholders=${showPlaceholders}`) })
  const [invite, setInvite] = useState(false)
  const update = async (id: string, body: Record<string, unknown>) => {
    try {
      await api.patch(`/users/${id}`, body)
      qc.invalidateQueries({ queryKey: ['admin-users'] })
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Failed')
    }
  }
  return (
    <div>
      <div className="row" style={{ marginBottom: 12 }}>
        <label className="row small">
          <input type="checkbox" checked={showPlaceholders} onChange={(e) => setShowPlaceholders(e.target.checked)} /> Show imported authors without account
        </label>
        <span className="spacer" />
        <button className="btn btn-primary" onClick={() => setInvite(true)}>
          <UserPlus size={15} /> Invite / add user
        </button>
      </div>
      <table className="list">
        <thead>
          <tr><th>Name</th><th>E-mail</th><th>Role</th><th>Status</th><th>Last login</th><th /></tr>
        </thead>
        <tbody>
          {users.data?.map((u) => (
            <tr key={u.id}>
              <td className="row"><Avatar name={u.name} id={u.id} /> {u.name}</td>
              <td className="muted">{u.email ?? '—'}</td>
              <td>
                {u.role === 'owner' ? <span className="badge badge-accent">owner</span> : (
                  <select className="select" style={{ width: 120, height: 28 }} value={u.role} onChange={(e) => update(u.id, { role: e.target.value })}>
                    <option value="admin">admin</option>
                    <option value="member">member</option>
                    <option value="guest">guest</option>
                  </select>
                )}
              </td>
              <td>
                {u.status === 'placeholder' ? <span className="badge">imported</span> : u.status === 'invited' ? (
                  <span className="row" style={{ gap: 6 }}>
                    <span className="badge">invited</span>
                    <button className="btn btn-ghost small" onClick={async () => {
                      try {
                        const r = await api.post<{ emailed: boolean; inviteUrl: string }>('/users/invite', { email: u.email, role: u.role })
                        if (r.emailed) toast(`Invitation sent again to ${u.email}`)
                        else { await navigator.clipboard?.writeText(r.inviteUrl).catch(() => {}); toast('E-mail is not configured — new invitation link copied') }
                      } catch (e) { toast(e instanceof Error ? e.message : 'Failed') }
                    }}>Resend</button>
                  </span>
                ) : u.role === 'owner' ? <span className="badge">active</span> : (
                  <select className="select" style={{ width: 130, height: 28 }} value={u.status} onChange={(e) => update(u.id, { status: e.target.value })}>
                    <option value="active">active</option>
                    <option value="deactivated">deactivated</option>
                  </select>
                )}
              </td>
              <td className="faint small">{formatDate(u.lastLoginAt) || 'never'}</td>
              <td>
                {u.role !== 'owner' && u.status === 'active' && (
                  <button className="btn btn-ghost small" title="Reset two-factor authentication (e.g. lost phone); signs the person out" onClick={async () => {
                    if (!confirm(`Reset two-factor authentication for ${u.name}? They'll be signed out and can sign in with their password only.`)) return
                    try { await api.post(`/users/${u.id}/reset-mfa`); toast(`Two-factor authentication reset for ${u.name}`) } catch (e) { toast(e instanceof Error ? e.message : 'Failed') }
                  }}>Reset 2FA</button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {invite && <AddUserDialog onClose={() => setInvite(false)} />}
    </div>
  )
}

function AddUserDialog({ onClose }: { onClose: () => void }) {
  const qc = useQueryClient()
  const [mode, setMode] = useState<'invite' | 'password'>('invite')
  const [form, setForm] = useState({ name: '', email: '', password: '', role: 'member' })
  const [error, setError] = useState<string | null>(null)
  const [invited, setInvited] = useState<{ emailed: boolean; inviteUrl: string } | null>(null)
  const submit = async () => {
    setError(null)
    try {
      if (mode === 'invite') {
        setInvited(await api.post<{ emailed: boolean; inviteUrl: string }>('/users/invite', { email: form.email, name: form.name || undefined, role: form.role }))
        qc.invalidateQueries({ queryKey: ['admin-users'] })
      } else {
        await api.post('/users', form)
        qc.invalidateQueries({ queryKey: ['admin-users'] })
        onClose()
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed')
    }
  }
  if (invited)
    return (
      <Modal title="Invitation created" onClose={onClose} footer={<button className="btn btn-primary" onClick={onClose}>Done</button>}>
        {invited.emailed ? (
          <p style={{ marginTop: 0 }}>An invitation was e-mailed to <b>{form.email}</b>. The link is valid for 7 days.</p>
        ) : (
          <p style={{ marginTop: 0 }}>E-mail isn't configured on this server, so share this invitation link with <b>{form.email}</b> yourself (valid for 7 days):</p>
        )}
        <div className="row">
          <input className="input" readOnly value={invited.inviteUrl} onFocus={(e) => e.target.select()} />
          <button className="btn" onClick={() => navigator.clipboard?.writeText(invited.inviteUrl)}>Copy</button>
        </div>
      </Modal>
    )
  return (
    <Modal
      title="Add people"
      onClose={onClose}
      footer={<><button className="btn" onClick={onClose}>Cancel</button><button className="btn btn-primary" onClick={submit}>{mode === 'invite' ? 'Send invitation' : 'Create'}</button></>}
    >
      <div className="tabs" style={{ marginTop: -6 }}>
        <button className={`tab ${mode === 'invite' ? 'active' : ''}`} onClick={() => setMode('invite')}>Invite by e-mail</button>
        <button className={`tab ${mode === 'password' ? 'active' : ''}`} onClick={() => setMode('password')}>Create with password</button>
      </div>
      <p className="muted small" style={{ marginTop: 0 }}>
        {mode === 'invite' ? 'They get an e-mail with a link to choose their name and password. ' : ''}
        If an imported Confluence author has this e-mail, the account is linked to their pages and comments.
      </p>
      <div className="field"><label>E-mail</label><input className="input" type="email" value={form.email} onChange={(e) => setForm({ ...form, email: e.target.value })} autoFocus /></div>
      <div className="field"><label>Name{mode === 'invite' ? ' (optional)' : ''}</label><input className="input" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} /></div>
      {mode === 'password' && <div className="field"><label>Initial password</label><input className="input" type="password" value={form.password} onChange={(e) => setForm({ ...form, password: e.target.value })} /></div>}
      <div className="field">
        <label>Role</label>
        <select className="select" value={form.role} onChange={(e) => setForm({ ...form, role: e.target.value })}>
          <option value="member">Member</option>
          <option value="admin">Admin</option>
          <option value="guest">Guest (only explicitly shared spaces)</option>
        </select>
      </div>
      {error && <p className="error-text">{error}</p>}
    </Modal>
  )
}

function GroupsAdmin() {
  const qc = useQueryClient()
  const groups = useQuery({ queryKey: ['groups'], queryFn: () => api.get<{ id: string; name: string; memberCount: number; members: { id: string; name: string }[] | null }[]>('/groups') })
  const users = useQuery({ queryKey: ['users-all'], queryFn: () => api.get<{ id: string; name: string }[]>('/users') })
  const [name, setName] = useState('')
  const [adding, setAdding] = useState<Record<string, string>>({})
  const refresh = () => qc.invalidateQueries({ queryKey: ['groups'] })
  return (
    <div>
      <div className="row" style={{ marginBottom: 16 }}>
        <input className="input" style={{ maxWidth: 280 }} placeholder="New group name" value={name} onChange={(e) => setName(e.target.value)} />
        <button className="btn btn-primary" disabled={!name.trim()} onClick={async () => { await api.post('/groups', { name }); setName(''); refresh() }}>
          <Users size={15} /> Create group
        </button>
      </div>
      {groups.data?.map((g) => (
        <div key={g.id} className="card" style={{ marginBottom: 10 }}>
          <div className="row">
            <b>{g.name}</b> <span className="badge">{g.memberCount} members</span>
            <span className="spacer" />
            <button className="btn btn-sm btn-danger" onClick={async () => { if (confirm(`Delete group ${g.name}?`)) { await api.del(`/groups/${g.id}`); refresh() } }}>Delete</button>
          </div>
          <div className="row" style={{ flexWrap: 'wrap', marginTop: 8 }}>
            {g.members?.map((m) => (
              <span key={m.id} className="label-chip row" style={{ gap: 4 }}>
                {m.name}
                <a style={{ cursor: 'pointer' }} onClick={async () => { await api.del(`/groups/${g.id}/members/${m.id}`); refresh() }}>×</a>
              </span>
            ))}
            <select className="select" style={{ width: 200, height: 28 }} value={adding[g.id] ?? ''} onChange={(e) => setAdding({ ...adding, [g.id]: e.target.value })}>
              <option value="">Add member…</option>
              {users.data?.filter((u) => !g.members?.some((m) => m.id === u.id)).map((u) => <option key={u.id} value={u.id}>{u.name}</option>)}
            </select>
            <button className="btn btn-sm" disabled={!adding[g.id]} onClick={async () => { await api.post(`/groups/${g.id}/members`, { userId: adding[g.id] }); setAdding({ ...adding, [g.id]: '' }); refresh() }}>Add</button>
          </div>
        </div>
      ))}
      {groups.data?.length === 0 && <div className="empty">No groups yet.</div>}
    </div>
  )
}

/** `attachmentBinariesMissing` / `skipped_page_draft` → "attachment binaries missing" / "skipped page draft" */
function humanize(k: string): string {
  return k.replace(/_/g, ' ').replace(/([a-z])([A-Z])/g, '$1 $2').toLowerCase()
}

function ReportView({ result }: { result: any }) {
  const created = result?.created ?? {}
  const links = result?.links ?? result?.conversion ?? {}
  const conv = result?.conversion ?? {}
  const macros: [string, [number, string | null]][] = Object.entries(conv.macros ?? {}) as any
  macros.sort((a, b) => b[1][0] - a[1][0])
  const counts = result?.report?.counts ?? {}
  return (
    <div>
      {result?.spaces && (
        <div className="row" style={{ flexWrap: 'wrap', marginBottom: 12 }}>
          {result.spaces.map((s: any) => (
            <Link key={s.key} className="btn" to={s.homePageId ? `/pages/${s.homePageId}` : `/spaces/${s.key}`}>
              Open {s.name ?? s.key} ({s.key})
            </Link>
          ))}
          {result.spaces.filter((s: any) => s.anonymousAccessInConfluence).map((s: any) => (
            <span key={s.key + '-anon'} className="badge" title="Imported spaces are never made public automatically">
              {s.key} was public in Confluence — enable anonymous access in space settings if wanted
            </span>
          ))}
        </div>
      )}
      <div className="grid-cards">
        {Object.entries({ ...counts, ...created }).filter(([, v]) => typeof v === 'number').map(([k, v]) => (
          <div key={k} className="stat"><div className="v">{String(v)}</div><div className="k">{humanize(k)}</div></div>
        ))}
      </div>
      <h4>Links and references</h4>
      <table className="list small">
        <tbody>
          {conv.page_links !== undefined && <tr><td>Page links resolved</td><td>{conv.page_links_resolved} / {conv.page_links}</td></tr>}
          {conv.attachment_refs !== undefined && <tr><td>Attachment references resolved</td><td>{conv.attachment_refs_resolved} / {conv.attachment_refs}</td></tr>}
          {conv.user_refs !== undefined && <tr><td>User mentions resolved</td><td>{conv.user_refs_resolved} / {conv.user_refs}</td></tr>}
          {links.inlineAnchors !== undefined && <tr><td>Inline comments anchored</td><td>{links.inlineAnchors}</td></tr>}
          {conv.lenient_parses !== undefined && <tr><td>Bodies needing lenient parsing</td><td>{conv.lenient_parses}</td></tr>}
        </tbody>
      </table>
      {macros.length > 0 && (
        <>
          <h4>Macros</h4>
          <table className="list small">
            <thead><tr><th>Macro</th><th>Count</th><th>Support</th></tr></thead>
            <tbody>
              {macros.map(([name, [n, support]]) => (
                <tr key={name}>
                  <td><code>{name}</code></td>
                  <td>{n}</td>
                  <td><span className={`badge ${support === 'native' ? 'badge-accent' : ''}`}>{support ?? '?'}</span></td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {result?.report?.warnings?.length > 0 && (
        <>
          <h4>Warnings</h4>
          <ul className="small">{result.report.warnings.slice(0, 50).map((w: string, i: number) => <li key={i}>{w}</li>)}</ul>
        </>
      )}
      {conv.missing_link_samples?.length > 0 && (
        <details>
          <summary className="small muted">Links to pages not contained in the export ({conv.page_links_missing})</summary>
          <ul className="small">{conv.missing_link_samples.map((s: string, i: number) => <li key={i}>{s}</li>)}</ul>
        </details>
      )}
    </div>
  )
}

function ImportAdmin() {
  const toast = useToast()
  const qc = useQueryClient()
  const file = useRef<HTMLInputElement>(null)
  const [selected, setSelected] = useState<File | null>(null)
  const [analysis, setAnalysis] = useState<any>(null)
  const [busy, setBusy] = useState<'analyze' | 'import' | null>(null)
  const [spaceKey, setSpaceKey] = useState('')
  const [skipHistory, setSkipHistory] = useState(false)
  const [jobId, setJobId] = useState<string | null>(null)
  const jobs = useQuery({ queryKey: ['jobs'], queryFn: () => api.get<Job[]>('/jobs'), refetchInterval: 5000 })
  const job = useQuery({ queryKey: ['job', jobId], queryFn: () => api.get<Job>(`/jobs/${jobId}`), enabled: !!jobId, refetchInterval: (q) => (q.state.data && ['succeeded', 'failed'].includes(q.state.data.status) ? false : 1000) })
  useEffect(() => {
    if (job.data?.status === 'succeeded') {
      qc.invalidateQueries({ queryKey: ['spaces'] })
    }
  }, [job.data?.status, qc])

  const analyze = async () => {
    if (!selected) return
    setBusy('analyze')
    setAnalysis(null)
    try {
      const form = new FormData()
      form.append('file', selected)
      setAnalysis(await api.upload('/imports/confluence/analyze', form))
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Analysis failed')
    } finally {
      setBusy(null)
    }
  }
  const start = async () => {
    if (!selected) return
    setBusy('import')
    try {
      const form = new FormData()
      form.append('options', JSON.stringify({ spaceKey: spaceKey || undefined, skipHistory }))
      form.append('file', selected)
      const r = await api.upload<{ jobId: string }>('/imports/confluence', form)
      setJobId(r.jobId)
      qc.invalidateQueries({ queryKey: ['jobs'] })
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Import failed')
    } finally {
      setBusy(null)
    }
  }
  const j = job.data
  return (
    <div>
      <div className="card" style={{ marginBottom: 18 }}>
        <h3 style={{ marginTop: 0 }}>Import from Confluence</h3>
        <p className="muted" style={{ marginTop: 0 }}>
          Upload a Confluence <b>space export</b> (.zip). Both the classic XML format (Server/Data Center, older Cloud) and the new Cloud CSV format are supported.
          Pages, hierarchy, history, attachments, comments (including inline comments), labels, restrictions and authors are imported; links and mentions are rewritten.
          Run an analysis first to see what will be imported.
        </p>
        <input ref={file} type="file" accept=".zip" hidden onChange={(e) => { setSelected(e.target.files?.[0] ?? null); setAnalysis(null) }} />
        <div className="row" style={{ flexWrap: 'wrap' }}>
          <button className="btn" onClick={() => file.current?.click()}><Upload size={15} /> {selected ? selected.name : 'Choose export (.zip)'}</button>
          {selected && <span className="faint small">{formatBytes(selected.size)}</span>}
          <span className="spacer" />
          <button className="btn" disabled={!selected || !!busy} onClick={analyze}>{busy === 'analyze' ? <Spinner /> : <FileSearch size={15} />} Analyze (dry run)</button>
        </div>
        {selected && (
          <div className="row" style={{ marginTop: 14, flexWrap: 'wrap' }}>
            <input className="input" style={{ width: 200 }} placeholder="Space key (optional)" value={spaceKey} onChange={(e) => setSpaceKey(e.target.value.toUpperCase())} />
            <label className="row small"><input type="checkbox" checked={skipHistory} onChange={(e) => setSkipHistory(e.target.checked)} /> Skip page history</label>
            <span className="spacer" />
            <button className="btn btn-primary" disabled={!!busy} onClick={start}>{busy === 'import' ? <Spinner /> : <Upload size={15} />} Start import</button>
          </div>
        )}
      </div>

      <MarkdownImport />

      <RestoreBackup onJob={(id) => { setJobId(id); qc.invalidateQueries({ queryKey: ['jobs'] }) }} />

      {j && (
        <div className="card" style={{ marginBottom: 18 }}>
          <div className="row">
            {j.status === 'succeeded' ? <CheckCircle2 color="var(--success)" size={18} /> : j.status === 'failed' ? <XCircle color="var(--danger)" size={18} /> : <Spinner />}
            <b>{j.kind === 'export.space' ? 'Export' : j.kind === 'import.backup' ? 'Restore' : 'Import'} {j.status}</b>
            <span className="faint small">{j.progress?.phase}{j.progress?.total ? ` · ${j.progress.done ?? 0}/${j.progress.total}` : ''}</span>
          </div>
          {j.progress?.total ? <div className="progress" style={{ marginTop: 10 }}><div style={{ width: `${Math.round(((j.progress.done ?? 0) / j.progress.total) * 100)}%` }} /></div> : null}
          {j.error && <p className="error-text">{j.error}</p>}
          {j.status === 'succeeded' && j.kind === 'import.backup' && (
            <p style={{ marginBottom: 0 }}>
              Restored <Link to={`/spaces/${j.result?.spaceKey}`}>{j.result?.spaceKey}</Link>: {j.result?.pages} pages, {j.result?.attachments} attachments, {j.result?.comments} comments
              {j.result?.unmappedRestrictions ? ` · ${j.result.unmappedRestrictions} restrictions referred to unknown people or groups and now allow only you` : ''}
            </p>
          )}
          {j.status === 'succeeded' && j.kind === 'export.space' && (
            <p style={{ marginBottom: 0 }}>{j.result?.file ? <a href={`/api/jobs/${j.id}/download`}>{j.result.filename}</a> : 'This export has expired.'}</p>
          )}
          {j.status === 'succeeded' && j.kind === 'import.confluence' && <div style={{ marginTop: 14 }}><ReportView result={j.result} /></div>}
        </div>
      )}

      {analysis && (
        <div className="card" style={{ marginBottom: 18 }}>
          <h3 style={{ marginTop: 0 }}>Analysis</h3>
          <p className="muted small">
            Format: <b>{analysis.format}</b> · source: {analysis.descriptor?.source ?? 'server'} · spaces: {analysis.spaces?.map((s: any) => `${s.name} (${s.key}, ${s.pages} pages)`).join(', ')} · {analysis.users} users
          </p>
          <ReportView result={analysis} />
        </div>
      )}

      <h3>Recent jobs</h3>
      <table className="list">
        <tbody>
          {jobs.data?.map((jj) => (
            <tr key={jj.id} style={{ cursor: 'pointer' }} onClick={() => setJobId(jj.id)}>
              <td><Database size={14} /> {jj.kind}</td>
              <td><span className={`badge ${jj.status === 'succeeded' ? 'badge-accent' : ''}`}>{jj.status}</span></td>
              <td className="faint small">{formatDate(jj.createdAt)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function Audit() {
  const audit = useQuery({ queryKey: ['audit'], queryFn: () => api.get<{ id: number; actorName: string | null; action: string; targetType: string | null; data: any; createdAt: string }[]>('/admin/audit') })
  return (
    <table className="list">
      <thead><tr><th>When</th><th>Who</th><th>Action</th><th>Details</th></tr></thead>
      <tbody>
        {audit.data?.map((a) => (
          <tr key={a.id}>
            <td className="faint small" title={a.createdAt}>{formatDate(a.createdAt)}</td>
            <td>{a.actorName ?? '—'}</td>
            <td><Activity size={13} /> <code>{a.action}</code></td>
            <td className="faint small" style={{ maxWidth: 420, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{JSON.stringify(a.data)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/** Admin → Import: restore a space from a WikiCraft backup archive. */
function RestoreBackup({ onJob }: { onJob: (id: string) => void }) {
  const toast = useToast()
  const file = useRef<HTMLInputElement>(null)
  const [selected, setSelected] = useState<File | null>(null)
  const [spaceKey, setSpaceKey] = useState('')
  const [busy, setBusy] = useState(false)
  const start = async () => {
    if (!selected) return
    setBusy(true)
    try {
      const form = new FormData()
      form.append('file', selected)
      if (spaceKey.trim()) form.append('spaceKey', spaceKey.trim())
      const r = await api.upload<{ jobId: string; spaceKey: string }>('/imports/backup', form)
      toast(`Restoring into space ${r.spaceKey}…`)
      onJob(r.jobId)
      setSelected(null)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Restore failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="card" style={{ marginBottom: 18 }}>
      <h3 style={{ marginTop: 0 }}>Restore a space backup</h3>
      <p className="muted" style={{ marginTop: 0 }}>
        Upload a <b>full backup</b> made under Space settings → Export. The space is restored as a new space with its pages, tree, labels, comments,
        attachments and restrictions; people and groups are matched by e-mail and name.
      </p>
      <input ref={file} type="file" accept=".zip" hidden onChange={(e) => setSelected(e.target.files?.[0] ?? null)} />
      <div className="row" style={{ flexWrap: 'wrap' }}>
        <button className="btn" onClick={() => file.current?.click()}><Upload size={15} /> {selected ? selected.name : 'Choose backup (.zip)'}</button>
        {selected && <span className="faint small">{formatBytes(selected.size)}</span>}
        <span className="spacer" />
        <input className="input" style={{ width: 220 }} placeholder="New space key (optional)" value={spaceKey} onChange={(e) => setSpaceKey(e.target.value.toUpperCase())} />
        <button className="btn btn-primary" disabled={!selected || busy} onClick={start}>{busy ? <Spinner /> : <RefreshCw size={15} />} Restore</button>
      </div>
    </div>
  )
}

/** Admin → Import: Word documents, Markdown folders, Obsidian vaults and Notion exports into a space. */
function MarkdownImport() {
  const toast = useToast()
  const file = useRef<HTMLInputElement>(null)
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<{ key: string; name: string }[]>('/spaces') })
  const [selected, setSelected] = useState<File | null>(null)
  const [space, setSpace] = useState('')
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ pages: number; attachments: number; space: string } | null>(null)
  const start = async () => {
    if (!selected || !space) return
    setBusy(true)
    setResult(null)
    try {
      const form = new FormData()
      form.append('spaceKey', space)
      form.append('file', selected)
      const r = await api.upload<{ pages: number; attachments: number }>('/imports/markdown', form)
      setResult({ ...r, space })
      setSelected(null)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Import failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="card" style={{ marginBottom: 18 }}>
      <h3 style={{ marginTop: 0 }}>Import Word, Markdown, Notion or Obsidian</h3>
      <p className="muted" style={{ marginTop: 0 }}>
        Upload a Word document (<b>.docx</b>), a <b>.md</b> file or a <b>.zip</b> of them: a Notion export (“Markdown &amp; CSV”), an Obsidian vault, a folder of Word
        files or any docs folder. Folders become page trees, images become attachments, links and <code>[[wiki links]]</code> are rewritten, Notion databases become
        tables, and front matter titles and tags are kept. Word headings, lists, tables, images, links, code and footnotes are converted.
      </p>
      <input ref={file} type="file" accept=".zip,.md,.markdown,.docx" hidden onChange={(e) => setSelected(e.target.files?.[0] ?? null)} />
      <div className="row" style={{ flexWrap: 'wrap' }}>
        <button className="btn" onClick={() => file.current?.click()}><Upload size={15} /> {selected ? selected.name : 'Choose file (.docx, .md or .zip)'}</button>
        {selected && <span className="faint small">{formatBytes(selected.size)}</span>}
        <span className="spacer" />
        <select className="select" style={{ width: 220 }} value={space} onChange={(e) => setSpace(e.target.value)}>
          <option value="">Into space…</option>
          {spaces.data?.map((s) => <option key={s.key} value={s.key}>{s.name}</option>)}
        </select>
        <button className="btn btn-primary" disabled={!selected || !space || busy} onClick={start}>{busy ? <Spinner /> : <Upload size={15} />} Import</button>
      </div>
      {result && (
        <p style={{ marginBottom: 0 }}>
          <CheckCircle2 size={15} color="var(--success)" style={{ verticalAlign: -2 }} /> Imported {result.pages} pages and {result.attachments} attachments into{' '}
          <Link to={`/spaces/${result.space}`}>{result.space}</Link>.
        </p>
      )}
    </div>
  )
}
