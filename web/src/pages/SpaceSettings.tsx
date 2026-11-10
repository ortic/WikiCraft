import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { useCanonicalSpaceKey } from '../useCanonicalKey'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Archive, Trash2, RotateCcw, Users, X } from 'lucide-react'
import { api, formatDate, type Shortcut, type Space, type SpaceLook } from '../api'
import { SymbolPicker } from '../icons'
import ExportPanel from '../components/ExportPanel'
import { ErrorBox, SpaceIcon, Spinner, useFeatures, useToast } from '../ui'
import { TemplateList } from '../components/TemplateManager'

interface Grant {
  subjectType: 'user' | 'group'
  subjectId: string
  role: string
  subjectName?: string | null
}

const ROLES = ['viewer', 'commenter', 'editor', 'admin']

export default function SpaceSettings() {
  const { key } = useParams()
  const [params, setParams] = useSearchParams()
  const requested = params.get('tab') ?? 'details'
  const features = useFeatures()
  const space = useQuery({ queryKey: ['space', key], queryFn: () => api.get<Space>(`/spaces/${key}`) })
  useCanonicalSpaceKey(key, space.data?.key)
  if (space.isLoading) return <div className="content-pad"><Spinner /></div>
  if (space.error) return <div className="content-pad"><ErrorBox error={space.error} /></div>
  const s = space.data!
  // non-admins have no details/permissions tabs
  const exports = features.exports !== false
  const fallback = exports ? 'export' : 'archived'
  const tab =
    (s.role !== 'admin' && (requested === 'details' || requested === 'permissions')) || (!exports && requested === 'export') ? fallback : requested
  return (
    <div className="content-pad">
      <h1 style={{ marginTop: 0 }}>{s.name} · Settings</h1>
      <div className="tabs">
        {s.role === 'admin' && (
          <>
            <button className={`tab ${tab === 'details' ? 'active' : ''}`} onClick={() => setParams({ tab: 'details' })}>Details</button>
            <button className={`tab ${tab === 'permissions' ? 'active' : ''}`} onClick={() => setParams({ tab: 'permissions' })}>Permissions</button>
          </>
        )}
        <button className={`tab ${tab === 'archived' ? 'active' : ''}`} onClick={() => setParams({ tab: 'archived' })}>Archived</button>
        <button className={`tab ${tab === 'trash' ? 'active' : ''}`} onClick={() => setParams({ tab: 'trash' })}>Trash</button>
        {exports && <button className={`tab ${tab === 'export' ? 'active' : ''}`} onClick={() => setParams({ tab: 'export' })}>Export</button>}
      </div>
      {tab === 'details' && s.role === 'admin' && <Details space={s} />}
      {tab === 'permissions' && s.role === 'admin' && <Permissions space={s} />}
      {tab === 'archived' && <ArchivedList space={s} />}
      {tab === 'trash' && <TrashList space={s} />}
      {tab === 'export' && <ExportPanel space={s} />}
    </div>
  )
}

function Details({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const [name, setName] = useState(space.name)
  const [description, setDescription] = useState(space.description ?? '')
  const [category, setCategory] = useState(space.category ?? '')
  const allSpaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const categories = [...new Set((allSpaces.data ?? []).map((s) => s.category).filter((c): c is string => !!c))].sort()
  const [icon, setIcon] = useState(space.icon ?? '')
  const [key, setKey] = useState(space.key)
  const [defaultTemplate, setDefaultTemplate] = useState((space as Space & { defaultTemplate?: string | null }).defaultTemplate ?? '')
  const templates = useQuery({ queryKey: ['templates', space.key], queryFn: () => api.get<{ id: string; name: string }[]>(`/templates?spaceKey=${encodeURIComponent(space.key)}`) })
  const nav = useNavigate()
  const save = async () => {
    const newKey = key.trim().toUpperCase()
    const renamed = newKey !== space.key
    if (renamed && !confirm(`Change the space key from ${space.key} to ${newKey}? Links with ${space.key} keep working.`)) return
    try {
      await api.patch(`/spaces/${space.key}`, { name, description, icon, category, defaultTemplate, ...(renamed ? { key: newKey } : {}) })
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not save')
      return
    }
    qc.invalidateQueries({ queryKey: ['spaces'] })
    qc.invalidateQueries({ queryKey: ['space'] })
    toast('Saved')
    if (renamed) nav(`/spaces/${newKey}/settings`, { replace: true })
  }
  return (
    <div style={{ maxWidth: 520 }}>
      <div className="field"><label>Name</label><input className="input" value={name} onChange={(e) => setName(e.target.value)} /></div>
      <div className="field">
        <label className="row">Icon <SpaceIcon space={{ key: space.key, name, icon }} size={26} /></label>
        <SymbolPicker value={icon || null} onChange={(v) => setIcon(v ?? '')} />
      </div>
      <div className="field"><label>Description</label><textarea className="textarea" value={description} onChange={(e) => setDescription(e.target.value)} /></div>
      <div className="field">
        <label>Category</label>
        <input className="input" style={{ maxWidth: 360 }} list="space-categories" value={category} maxLength={60} placeholder="e.g. Teams, Projects, Knowledge base" onChange={(e) => setCategory(e.target.value)} />
        <datalist id="space-categories">{categories.map((c) => <option key={c} value={c} />)}</datalist>
        <div className="faint small" style={{ marginTop: 4 }}>Groups the space in the space directory.</div>
      </div>
      <div className="field">
        <label>New pages start from</label>
        <select className="select" style={{ maxWidth: 360 }} value={defaultTemplate} onChange={(e) => setDefaultTemplate(e.target.value)}>
          <option value="">A blank page</option>
          {templates.data?.map((t) => <option key={t.id} value={t.id}>{t.name}</option>)}
        </select>
      </div>
      <div className="field">
        <label>Key</label>
        <input className="input" value={key} maxLength={32} onChange={(e) => setKey(e.target.value.toUpperCase().replace(/[^A-Z0-9_-]/g, ''))} />
        {key !== space.key && <div className="faint small">Pages keep their links: URLs with {space.key} redirect to {key || '…'}.</div>}
      </div>
      <button className="btn btn-primary" onClick={save}>Save</button>
      <Shortcuts space={space} />
      <SpaceLookCard space={space} />
      <div className="card" style={{ marginTop: 28 }}>
        <b>Templates</b>
        <p className="muted small" style={{ margin: '4px 0 10px' }}>This space’s own templates, offered next to the built-in ones and those for every space.</p>
        <TemplateList spaceKey={space.key} />
      </div>
      {!space.personalOwnerId && <SpaceOwner space={space} />}
      {!space.personalOwnerId && <CopySpace space={space} />}
      <div className="card danger-zone" style={{ marginTop: 28 }}>
        <b>Archive this space</b>
        <p className="muted small" style={{ margin: '4px 0 10px' }}>Archived spaces are read-only and leave search and the space list. Workspace admins can restore them under Admin → Spaces.</p>
        <button
          className="btn"
          onClick={async () => {
            if (!confirm(`Archive “${space.name}”?`)) return
            await api.del(`/spaces/${space.key}`)
            qc.invalidateQueries({ queryKey: ['spaces'] })
            location.href = '/spaces'
          }}
        >
          Archive space
        </button>
      </div>
    </div>
  )
}

/** Links pinned to the space sidebar. */
function Shortcuts({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const [rows, setRows] = useState<Shortcut[] | null>(null)
  const list = rows ?? space.shortcuts ?? []
  const set = (i: number, k: keyof Shortcut, v: string) => setRows(list.map((r, j) => (j === i ? { ...r, [k]: v } : r)))
  const save = async () => {
    try {
      await api.patch(`/spaces/${space.key}`, { shortcuts: list.filter((r) => r.title.trim() || r.url.trim()) })
      qc.invalidateQueries({ queryKey: ['space'] })
      qc.invalidateQueries({ queryKey: ['spaces'] })
      setRows(null)
      toast('Shortcuts saved')
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not save')
    }
  }
  return (
    <div className="card" style={{ marginTop: 28 }}>
      <b>Sidebar shortcuts</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>
        Links under the space name in the sidebar: a web address (https://…) or a path in this wiki (/pages/…, /labels/…). The page menu can add the current page.
      </p>
      {list.map((r, i) => (
        <div key={i} className="row" style={{ gap: 6, marginBottom: 6 }}>
          <input className="input" style={{ flex: '1 1 140px' }} aria-label={`Title of shortcut ${i + 1}`} placeholder="Title" maxLength={80} value={r.title} onChange={(e) => set(i, 'title', e.target.value)} />
          <input className="input" style={{ flex: '2 1 200px' }} aria-label={`Link of shortcut ${i + 1}`} placeholder="https://… or /pages/…" value={r.url} onChange={(e) => set(i, 'url', e.target.value)} />
          <button className="icon-btn" title="Remove" aria-label={`Remove shortcut ${i + 1}`} onClick={() => setRows(list.filter((_, j) => j !== i))}>
            <X size={14} />
          </button>
        </div>
      ))}
      <div className="row" style={{ gap: 6 }}>
        {list.length < 20 && <button className="btn btn-sm" onClick={() => setRows([...list, { title: '', url: '' }])}>Add shortcut</button>}
        <span className="spacer" />
        <button className="btn btn-primary btn-sm" disabled={!rows} onClick={save}>Save</button>
      </div>
    </div>
  )
}

/** The space's own accent colour and sidebar options. */
function SpaceLookCard({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const look = space.look ?? {}
  const [color, setColor] = useState<string | null>(null)
  const shown = color ?? look.accentColor ?? ''
  const valid = /^#[0-9a-f]{6}$/i.test(shown)
  const save = async (next: SpaceLook, message: string) => {
    try {
      await api.patch(`/spaces/${space.key}`, { look: next })
      qc.invalidateQueries({ queryKey: ['space'] })
      qc.invalidateQueries({ queryKey: ['spaces'] })
      setColor(null)
      toast(message)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not save')
    }
  }
  return (
    <div className="card" style={{ marginTop: 28 }}>
      <b>Look</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>An accent colour for buttons, links and highlights while people are in this space; without one, the workspace’s.</p>
      <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
        <input type="color" aria-label="Pick a colour" value={valid ? shown : '#4f46e5'} onChange={(e) => setColor(e.target.value)} style={{ width: 40, height: 32, padding: 2, border: '1px solid var(--border)', borderRadius: 6, background: 'var(--bg)' }} />
        <input className="input" aria-label="Accent colour" style={{ maxWidth: 120, fontFamily: 'var(--mono)' }} placeholder="Workspace’s" maxLength={7} value={shown} onChange={(e) => setColor(e.target.value)} />
        <button className="btn btn-sm" disabled={!valid || shown.toLowerCase() === (look.accentColor ?? '')} onClick={() => save({ ...look, accentColor: shown }, 'Colour saved')}>Save</button>
        {look.accentColor && <button className="btn btn-sm" onClick={() => save({ ...look, accentColor: '' }, 'Back to the workspace colour')}>Use the workspace colour</button>}
      </div>
      <label className="row" style={{ gap: 8, marginTop: 12 }}>
        <input type="checkbox" checked={!look.hideBlog} onChange={(e) => save({ ...look, hideBlog: !e.target.checked }, e.target.checked ? 'The blog is back in the sidebar' : 'The blog is hidden from the sidebar')} />
        Show the blog in the sidebar
      </label>
    </div>
  )
}

/** A new space with this one's pages, settings, templates and people. */
function CopySpace({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const nav = useNavigate()
  const [open, setOpen] = useState(false)
  const [name, setName] = useState(`${space.name} (copy)`)
  const [key, setKey] = useState(`${space.key}2`.slice(0, 32))
  const [attachments, setAttachments] = useState(true)
  const [people, setPeople] = useState(true)
  const [busy, setBusy] = useState(false)
  const run = async () => {
    setBusy(true)
    try {
      const r = await api.post<{ key: string; pages: number; truncated: boolean }>(`/spaces/${space.key}/copy`, { key, name, includeAttachments: attachments, includePermissions: people })
      qc.invalidateQueries({ queryKey: ['spaces'] })
      toast(r.truncated ? `Copied the first ${r.pages} pages` : `Copied ${r.pages} pages`)
      nav(`/spaces/${r.key}`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not copy the space')
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="card" style={{ marginTop: 28 }}>
      <b>Copy this space</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>
        A new space with the published pages you can see (not drafts or archived pages), their labels, properties and restrictions, and this space's settings and
        templates. Links between the pages point at the copies.
      </p>
      {!open ? (
        <button className="btn btn-sm" onClick={() => setOpen(true)}>Copy space…</button>
      ) : (
        <>
          <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
            <div className="field" style={{ flex: '1 1 220px', marginBottom: 8 }}>
              <label htmlFor="copy-name">Name</label>
              <input id="copy-name" className="input" value={name} onChange={(e) => setName(e.target.value)} />
            </div>
            <div className="field" style={{ flex: '0 1 160px', marginBottom: 8 }}>
              <label htmlFor="copy-key">Key</label>
              <input id="copy-key" className="input" value={key} maxLength={32} onChange={(e) => setKey(e.target.value.toUpperCase().replace(/[^A-Z0-9_-]/g, ''))} />
            </div>
          </div>
          <label className="row small"><input type="checkbox" checked={attachments} onChange={(e) => setAttachments(e.target.checked)} /> With attachments</label>
          <label className="row small"><input type="checkbox" checked={people} onChange={(e) => setPeople(e.target.checked)} /> With the people and groups of this space</label>
          <div className="row" style={{ gap: 6, marginTop: 10 }}>
            <button className="btn btn-primary btn-sm" disabled={busy || !name.trim() || !key} onClick={run}>{busy ? <Spinner /> : null} Copy</button>
            <button className="btn btn-sm" onClick={() => setOpen(false)}>Cancel</button>
          </div>
        </>
      )}
    </div>
  )
}

/** Who is responsible for the space, and handing it over. */
function SpaceOwner({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const [picking, setPicking] = useState(false)
  const [q, setQ] = useState('')
  const users = useQuery({
    queryKey: ['user-search', q],
    queryFn: () => api.get<{ id: string; name: string; role?: string }[]>(`/users?q=${encodeURIComponent(q)}`),
    enabled: picking && q.trim().length > 0,
  })
  const pick = async (u: { id: string; name: string }) => {
    if (!confirm(`Make ${u.name} the owner of “${space.name}”? They become a space admin; your own role stays as it is.`)) return
    try {
      await api.put(`/spaces/${space.key}/owner`, { userId: u.id })
      qc.invalidateQueries({ queryKey: ['space'] })
      setPicking(false)
      setQ('')
      toast(`${u.name} owns this space now`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not change the owner')
    }
  }
  return (
    <div className="card" style={{ marginTop: 28 }}>
      <b>Owner</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>
        The person responsible for this space and the first contact for questions about it. The owner is always a space admin.
      </p>
      <div className="row" style={{ gap: 8 }}>
        {space.ownerId && space.ownerName ? <Link to={`/people/${space.ownerId}`}>{space.ownerName}</Link> : <span className="faint">Nobody</span>}
        <span className="spacer" />
        {!picking && <button className="btn btn-sm" onClick={() => setPicking(true)}>Change owner…</button>}
      </div>
      {picking && (
        <div style={{ marginTop: 10 }}>
          <div className="row" style={{ gap: 6 }}>
            <input className="input" autoFocus placeholder="Search people" aria-label="New owner" value={q} onChange={(e) => setQ(e.target.value)} />
            <button className="btn btn-sm" onClick={() => { setPicking(false); setQ('') }}>Cancel</button>
          </div>
          <div className="list-plain" role="listbox" style={{ marginTop: 4 }}>
            {(users.data ?? []).filter((u) => u.id !== space.ownerId && u.role !== 'guest').slice(0, 8).map((u) => (
              <button key={u.id} type="button" role="option" aria-selected={false} className="menu-item" style={{ width: '100%' }} onClick={() => pick(u)}>
                {u.name}
              </button>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}

function Permissions({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const perms = useQuery({ queryKey: ['space-perms', space.key], queryFn: () => api.get<{ defaultRole: string; anonymousAccess: boolean; grants: Grant[] }>(`/spaces/${space.key}/permissions`) })
  const users = useQuery({ queryKey: ['users-all'], queryFn: () => api.get<{ id: string; name: string }[]>('/users') })
  const groups = useQuery({ queryKey: ['groups'], queryFn: () => api.get<{ id: string; name: string }[]>('/groups') })
  const [grants, setGrants] = useState<Grant[]>([])
  const [defaultRole, setDefaultRole] = useState('editor')
  const [anon, setAnon] = useState(false)
  const [pick, setPick] = useState('')
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (perms.data) {
      setGrants(perms.data.grants)
      setDefaultRole(perms.data.defaultRole)
      setAnon(perms.data.anonymousAccess)
    }
  }, [perms.data])
  const save = async () => {
    setError(null)
    try {
      await api.put(`/spaces/${space.key}/permissions`, { defaultRole, anonymousAccess: anon, grants })
      qc.invalidateQueries()
      toast('Permissions saved')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed')
    }
  }
  const add = () => {
    const [type, id] = pick.split(':')
    const name = type === 'user' ? users.data?.find((u) => u.id === id)?.name : groups.data?.find((g) => g.id === id)?.name
    if (!id || grants.some((g) => g.subjectId === id)) return
    setGrants([...grants, { subjectType: type as 'user' | 'group', subjectId: id, role: 'editor', subjectName: name }])
    setPick('')
  }
  if (perms.isLoading) return <Spinner />
  return (
    <div style={{ maxWidth: 720 }}>
      <div className="field">
        <label>All workspace members (except guests)</label>
        <select className="select" value={defaultRole} onChange={(e) => setDefaultRole(e.target.value)} style={{ maxWidth: 320 }}>
          <option value="none">No access</option>
          {ROLES.map((r) => <option key={r} value={r}>{r}</option>)}
        </select>
      </div>
      <label className="row" style={{ marginBottom: 18 }}>
        <input type="checkbox" checked={anon} onChange={(e) => setAnon(e.target.checked)} /> Anyone on the internet can view this space (anonymous access)
      </label>
      <h3>People and groups</h3>
      <div className="row" style={{ marginBottom: 10 }}>
        <select className="select" value={pick} onChange={(e) => setPick(e.target.value)} style={{ maxWidth: 360 }}>
          <option value="">Add a person or group…</option>
          {groups.data?.map((g) => <option key={g.id} value={`group:${g.id}`}>{g.name} (group)</option>)}
          {users.data?.map((u) => <option key={u.id} value={`user:${u.id}`}>{u.name}</option>)}
        </select>
        <button className="btn" onClick={add} disabled={!pick}>Add</button>
      </div>
      <table className="list">
        <tbody>
          {grants.map((g, i) => (
            <tr key={g.subjectType + g.subjectId}>
              <td className="row">{g.subjectType === 'group' && <Users size={15} />} {g.subjectName ?? g.subjectId}</td>
              <td style={{ width: 180 }}>
                <select className="select" value={g.role} onChange={(e) => setGrants(grants.map((x, j) => (j === i ? { ...x, role: e.target.value } : x)))}>
                  {ROLES.map((r) => <option key={r} value={r}>{r}</option>)}
                </select>
              </td>
              <td style={{ width: 40 }}>
                <button className="icon-btn" onClick={() => setGrants(grants.filter((_, j) => j !== i))}><X size={14} /></button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {error && <p className="error-text">{error}</p>}
      <button className="btn btn-primary" style={{ marginTop: 16 }} onClick={save}>Save permissions</button>
    </div>
  )
}

function ArchivedList({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const list = useQuery({
    queryKey: ['archived', space.key],
    queryFn: () => api.get<{ id: string; title: string; archivedAt: string; archivedBy: string | null; descendants: number; canRestore: boolean }[]>(`/spaces/${space.key}/archived`),
  })
  return (
    <div>
      <p className="muted" style={{ marginTop: 0 }}>Archived pages are read-only and hidden from the page tree and search. Their links keep working.</p>
      {list.data?.length === 0 && <div className="empty"><Archive size={28} /><br />No archived pages.</div>}
      <table className="list">
        <tbody>
          {list.data?.map((t) => (
            <tr key={t.id}>
              <td><Link to={`/pages/${t.id}`}><b>{t.title}</b></Link>{t.descendants > 0 && <span className="faint small"> and {t.descendants} page{t.descendants === 1 ? '' : 's'} under it</span>}</td>
              <td className="faint small">Archived {formatDate(t.archivedAt)} by {t.archivedBy ?? 'someone'}</td>
              <td style={{ width: 120 }}>
                {t.canRestore && (
                  <button
                    className="btn btn-sm"
                    onClick={async () => {
                      await api.post(`/pages/${t.id}/unarchive`)
                      qc.invalidateQueries({ queryKey: ['archived', space.key] })
                      qc.invalidateQueries({ queryKey: ['tree'] })
                      qc.invalidateQueries({ queryKey: ['page', t.id] })
                      toast('Restored from the archive')
                    }}
                  >
                    <RotateCcw size={13} /> Restore
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function TrashList({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const trash = useQuery({ queryKey: ['trash', space.key], queryFn: () => api.get<{ id: string; title: string; deletedAt: string; deletedByName: string | null }[]>(`/spaces/${space.key}/trash`) })
  const isAdmin = space.role === 'admin'
  const refresh = () => qc.invalidateQueries({ queryKey: ['trash', space.key] })
  const purge = async (id: string, title: string) => {
    if (!confirm(`Delete “${title}” for good? Its history, comments, attachments and the trashed pages below it go too. This can't be undone.`)) return
    try {
      const r = await api.post<{ purged: number }>(`/pages/${id}/purge`)
      refresh()
      toast(r.purged > 1 ? `Deleted ${r.purged} pages for good` : 'Deleted for good')
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not delete')
    }
  }
  const empty = async () => {
    if (!confirm(`Delete everything in the trash of ${space.name} for good? This can't be undone.`)) return
    try {
      const r = await api.post<{ purged: number; kept: number }>(`/spaces/${space.key}/trash/empty`)
      refresh()
      toast(`Deleted ${r.purged} page${r.purged === 1 ? '' : 's'} for good${r.kept ? ` · ${r.kept} restricted ones you can't see stay` : ''}`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not empty the trash')
    }
  }
  return (
    <div>
      {isAdmin && !!trash.data?.length && (
        <div className="row" style={{ marginBottom: 12 }}>
          <span className="faint small">Restore pages, or delete them for good.</span>
          <span className="spacer" />
          <button className="btn btn-danger" onClick={empty}><Trash2 size={14} /> Empty trash</button>
        </div>
      )}
      {trash.data?.length === 0 && <div className="empty"><Trash2 size={28} /><br />Trash is empty.</div>}
      <table className="list">
        <tbody>
          {trash.data?.map((t) => (
            <tr key={t.id}>
              <td><b>{t.title}</b></td>
              <td className="faint small">Deleted {formatDate(t.deletedAt)} by {t.deletedByName ?? 'someone'}</td>
              <td style={{ width: isAdmin ? 250 : 120 }}>
                <div className="row" style={{ gap: 6, justifyContent: 'flex-end' }}>
                  <button
                    className="btn btn-sm"
                    onClick={async () => {
                      await api.post(`/pages/${t.id}/restore`)
                      qc.invalidateQueries({ queryKey: ['trash', space.key] })
                      qc.invalidateQueries({ queryKey: ['tree', space.key] })
                      toast('Restored')
                    }}
                  >
                    <RotateCcw size={13} /> Restore
                  </button>
                  {isAdmin && (
                    <button className="btn btn-sm btn-danger" onClick={() => purge(t.id, t.title)}>
                      Delete forever
                    </button>
                  )}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
