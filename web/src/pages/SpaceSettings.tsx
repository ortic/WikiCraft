import { t as tr } from '../i18n'
import { Fragment, useEffect, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { useCanonicalSpaceKey } from '../useCanonicalKey'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Archive, Trash2, RotateCcw, Users, X, Unlink, Link2 } from 'lucide-react'
import { api, can, formatDate, type LinkReport, type Perm, type Shortcut, type Space, type SpaceLook, spaceRoleName } from '../api'
import { SymbolPicker } from '../icons'
import ExportPanel from '../components/ExportPanel'
import { ErrorBox, SpaceIcon, Spinner, useFeatures, useToast } from '../ui'
import { TemplateList } from '../components/TemplateManager'

interface Grant {
  subjectType: 'user' | 'group'
  subjectId: string
  role: string
  /** permissions of the role this grant leaves out */
  denied?: Perm[]
  subjectName?: string | null
}

const ROLES = ['viewer', 'commenter', 'editor', 'admin']
const RANK: Record<string, number> = { none: 0, viewer: 1, commenter: 2, editor: 3, admin: 4 }

/** What each space permission lets people do (the server's `authz::Perm`). */
const PERM_LABELS: Record<Perm, string> = {
  'page.add': tr('Add pages'),
  'page.delete': tr('Delete pages'),
  'blog.add': tr('Add blog posts'),
  'blog.delete': tr('Delete blog posts'),
  'comment.add': tr('Add comments'),
  'comment.delete': tr('Delete others\' comments'),
  'attachment.add': tr('Add files'),
  'attachment.delete': tr('Delete files'),
  restrict: tr('Set page restrictions'),
  export: tr('Export'),
  archive: tr('Archive pages'),
  'delete.own': tr('Delete own content'),
}

interface PermInfo {
  key: Perm
  minRole: string
  confluence: string
}

/** The permissions a role brings, each one a checkbox: unchecked ones the grant leaves out. */
function PermissionChecks({ role, denied, catalog, onChange, label }: { role: string; denied: Perm[]; catalog: PermInfo[]; onChange: (denied: Perm[]) => void; label: string }) {
  const brought = catalog.filter((p) => RANK[role] >= RANK[p.minRole])
  if (!brought.length) return null
  return (
    <fieldset className="perm-checks" aria-label={tr('Permissions of {label}', { label })}>
      {brought.map((p) => (
        <label key={p.key} className="row" title={tr('Confluence: {confluence}', { confluence: p.confluence })}>
          <input
            type="checkbox"
            checked={!denied.includes(p.key)}
            onChange={(e) => onChange(e.target.checked ? denied.filter((d) => d !== p.key) : [...denied, p.key])}
          />{' '}
          {PERM_LABELS[p.key] ?? p.key}
        </label>
      ))}
    </fieldset>
  )
}

/** "All permissions" or how many the grant leaves out; opens the checklist. */
function PermissionSummary({ role, denied, catalog, open, onToggle }: { role: string; denied: Perm[]; catalog: PermInfo[]; open: boolean; onToggle: () => void }) {
  if (role === 'admin') return <span className="faint small">{tr('All permissions')}</span>
  const brought = catalog.filter((p) => RANK[role] >= RANK[p.minRole])
  if (!brought.length) return <span className="faint small">{tr('View only')}</span>
  const kept = brought.filter((p) => !denied.includes(p.key)).length
  const text = kept === brought.length ? tr('All permissions') : role === 'viewer' && kept === 0 ? tr('View only') : tr('{kept} of {length} permissions', { kept, length: brought.length })
  return (
    <button type="button" className="link-btn small" style={{ whiteSpace: 'nowrap' }} aria-expanded={open} onClick={onToggle}>
      {text}
    </button>
  )
}

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
  const exports = features.exports !== false && can(s, 'export')
  const fallback = exports ? 'export' : 'archived'
  const tab =
    (s.role !== 'admin' && (requested === 'details' || requested === 'permissions')) || (!exports && requested === 'export') ? fallback : requested
  return (
    <div className="content-pad">
      <h1 style={{ marginTop: 0 }}>{tr('{name} · Settings', { name: s.name })}</h1>
      <div className="tabs">
        {s.role === 'admin' && (
          <>
            <button className={`tab ${tab === 'details' ? 'active' : ''}`} onClick={() => setParams({ tab: 'details' })}>{tr('Details')}</button>
            <button className={`tab ${tab === 'permissions' ? 'active' : ''}`} onClick={() => setParams({ tab: 'permissions' })}>{tr('Permissions')}</button>
          </>
        )}
        <button className={`tab ${tab === 'links' ? 'active' : ''}`} onClick={() => setParams({ tab: 'links' })}>{tr('Links')}</button>
        <button className={`tab ${tab === 'archived' ? 'active' : ''}`} onClick={() => setParams({ tab: 'archived' })}>{tr('Archived')}</button>
        <button className={`tab ${tab === 'trash' ? 'active' : ''}`} onClick={() => setParams({ tab: 'trash' })}>{tr('Trash')}</button>
        {exports && <button className={`tab ${tab === 'export' ? 'active' : ''}`} onClick={() => setParams({ tab: 'export' })}>{tr('Export')}</button>}
      </div>
      {tab === 'details' && s.role === 'admin' && <Details space={s} />}
      {tab === 'permissions' && s.role === 'admin' && <Permissions space={s} />}
      {tab === 'links' && <LinkReportPanel space={s} />}
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
    if (renamed && !confirm(tr('Change the space key from {key} to {newKey}? Links with {key} keep working.', { key: space.key, newKey }))) return
    try {
      await api.patch(`/spaces/${space.key}`, { name, description, icon, category, defaultTemplate, ...(renamed ? { key: newKey } : {}) })
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
      return
    }
    qc.invalidateQueries({ queryKey: ['spaces'] })
    qc.invalidateQueries({ queryKey: ['space'] })
    toast(tr('Saved'))
    if (renamed) nav(`/spaces/${newKey}/settings`, { replace: true })
  }
  return (
    <div style={{ maxWidth: 520 }}>
      <div className="field"><label>{tr('Name')}</label><input className="input" value={name} onChange={(e) => setName(e.target.value)} /></div>
      <div className="field">
        <label className="row">{tr('Icon')}{' '}<SpaceIcon space={{ key: space.key, name, icon }} size={26} /></label>
        <SymbolPicker value={icon || null} onChange={(v) => setIcon(v ?? '')} />
      </div>
      <div className="field"><label>{tr('Description')}</label><textarea className="textarea" value={description} onChange={(e) => setDescription(e.target.value)} /></div>
      <div className="field">
        <label>{tr('Category')}</label>
        <input className="input" style={{ maxWidth: 360 }} list="space-categories" value={category} maxLength={60} placeholder={tr('e.g. Teams, Projects, Knowledge base')} onChange={(e) => setCategory(e.target.value)} />
        <datalist id="space-categories">{categories.map((c) => <option key={c} value={c} />)}</datalist>
        <div className="faint small" style={{ marginTop: 4 }}>{tr('Groups the space in the space directory.')}</div>
      </div>
      <div className="field">
        <label>{tr('New pages start from')}</label>
        <select className="select" style={{ maxWidth: 360 }} value={defaultTemplate} onChange={(e) => setDefaultTemplate(e.target.value)}>
          <option value="">{tr('A blank page')}</option>
          {templates.data?.map((t) => <option key={t.id} value={t.id}>{t.name}</option>)}
        </select>
      </div>
      <div className="field">
        <label>{tr('Key')}</label>
        <input className="input" value={key} maxLength={32} onChange={(e) => setKey(e.target.value.toUpperCase().replace(/[^A-Z0-9_-]/g, ''))} />
        {key !== space.key && <div className="faint small">{tr('Pages keep their links: URLs with {key} redirect to {value}.', { key: space.key, value: key || '…' })}</div>}
      </div>
      <button className="btn btn-primary" onClick={save}>{tr('Save')}</button>
      <Shortcuts space={space} />
      <SpaceLookCard space={space} />
      <div className="card" style={{ marginTop: 28 }}>
        <b>{tr('Templates')}</b>
        <p className="muted small" style={{ margin: '4px 0 10px' }}>{tr('This space’s own templates, offered next to the built-in ones and those for every space.')}</p>
        <TemplateList spaceKey={space.key} />
      </div>
      {!space.personalOwnerId && <SpaceOwner space={space} />}
      {!space.personalOwnerId && <CopySpace space={space} />}
      <div className="card danger-zone" style={{ marginTop: 28 }}>
        <b>{tr('Archive this space')}</b>
        <p className="muted small" style={{ margin: '4px 0 10px' }}>{tr('Archived spaces are read-only and leave search and the space list. Workspace admins can restore them under Admin → Spaces.')}</p>
        <button
          className="btn"
          onClick={async () => {
            if (!confirm(tr('Archive “{name}”?', { name: space.name }))) return
            await api.del(`/spaces/${space.key}`)
            qc.invalidateQueries({ queryKey: ['spaces'] })
            location.href = '/spaces'
          }}
        >{tr('Archive space')}</button>
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
      toast(tr('Shortcuts saved'))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  return (
    <div className="card" style={{ marginTop: 28 }}>
      <b>{tr('Sidebar shortcuts')}</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>{tr('Links under the space name in the sidebar: a web address (https://…) or a path in this wiki (/pages/…, /labels/…). The page menu can add the current page.')}</p>
      {list.map((r, i) => (
        <div key={i} className="row" style={{ gap: 6, marginBottom: 6 }}>
          <input className="input" style={{ flex: '1 1 140px' }} aria-label={tr('Title of shortcut {value}', { value: i + 1 })} placeholder={tr('Title')} maxLength={80} value={r.title} onChange={(e) => set(i, 'title', e.target.value)} />
          <input className="input" style={{ flex: '2 1 200px' }} aria-label={tr('Link of shortcut {value}', { value: i + 1 })} placeholder={tr('https://… or /pages/…')} value={r.url} onChange={(e) => set(i, 'url', e.target.value)} />
          <button className="icon-btn" title={tr('Remove')} aria-label={tr('Remove shortcut {value}', { value: i + 1 })} onClick={() => setRows(list.filter((_, j) => j !== i))}>
            <X size={14} />
          </button>
        </div>
      ))}
      <div className="row" style={{ gap: 6 }}>
        {list.length < 20 && <button className="btn btn-sm" onClick={() => setRows([...list, { title: '', url: '' }])}>{tr('Add shortcut')}</button>}
        <span className="spacer" />
        <button className="btn btn-primary btn-sm" disabled={!rows} onClick={save}>{tr('Save')}</button>
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
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  return (
    <div className="card" style={{ marginTop: 28 }}>
      <b>{tr('Look')}</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>{tr('An accent colour for buttons, links and highlights while people are in this space; without one, the workspace’s.')}</p>
      <div className="row" style={{ gap: 6, flexWrap: 'wrap' }}>
        <input type="color" aria-label={tr('Pick a colour')} value={valid ? shown : '#4f46e5'} onChange={(e) => setColor(e.target.value)} style={{ width: 40, height: 32, padding: 2, border: '1px solid var(--border)', borderRadius: 6, background: 'var(--bg)' }} />
        <input className="input" aria-label={tr('Accent colour')} style={{ maxWidth: 120, fontFamily: 'var(--mono)' }} placeholder={tr('Workspace’s')} maxLength={7} value={shown} onChange={(e) => setColor(e.target.value)} />
        <button className="btn btn-sm" disabled={!valid || shown.toLowerCase() === (look.accentColor ?? '')} onClick={() => save({ ...look, accentColor: shown }, tr('Colour saved'))}>{tr('Save')}</button>
        {look.accentColor && <button className="btn btn-sm" onClick={() => save({ ...look, accentColor: '' }, tr('Back to the workspace colour'))}>{tr('Use the workspace colour')}</button>}
      </div>
      <label className="row" style={{ gap: 8, marginTop: 12 }}>
        <input type="checkbox" checked={!look.hideBlog} onChange={(e) => save({ ...look, hideBlog: !e.target.checked }, e.target.checked ? tr('The blog is back in the sidebar') : tr('The blog is hidden from the sidebar'))} />{tr('Show the blog in the sidebar')}</label>
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
      toast(r.truncated ? tr('Copied the first {pages} pages', { pages: r.pages }) : tr('Copied {pages} pages', { pages: r.pages }))
      nav(`/spaces/${r.key}`)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not copy the space'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="card" style={{ marginTop: 28 }}>
      <b>{tr('Copy this space')}</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>{tr('A new space with the published pages you can see (not drafts or archived pages), their labels, properties and restrictions, and this space\'s settings and templates. Links between the pages point at the copies.')}</p>
      {!open ? (
        <button className="btn btn-sm" onClick={() => setOpen(true)}>{tr('Copy space…')}</button>
      ) : (
        <>
          <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
            <div className="field" style={{ flex: '1 1 220px', marginBottom: 8 }}>
              <label htmlFor="copy-name">{tr('Name')}</label>
              <input id="copy-name" className="input" value={name} onChange={(e) => setName(e.target.value)} />
            </div>
            <div className="field" style={{ flex: '0 1 160px', marginBottom: 8 }}>
              <label htmlFor="copy-key">{tr('Key')}</label>
              <input id="copy-key" className="input" value={key} maxLength={32} onChange={(e) => setKey(e.target.value.toUpperCase().replace(/[^A-Z0-9_-]/g, ''))} />
            </div>
          </div>
          <label className="row small"><input type="checkbox" checked={attachments} onChange={(e) => setAttachments(e.target.checked)} />{' '}{tr('With attachments')}</label>
          <label className="row small"><input type="checkbox" checked={people} onChange={(e) => setPeople(e.target.checked)} />{' '}{tr('With the people and groups of this space')}</label>
          <div className="row" style={{ gap: 6, marginTop: 10 }}>
            <button className="btn btn-primary btn-sm" disabled={busy || !name.trim() || !key} onClick={run}>{busy ? <Spinner /> : null}{' '}{tr('Copy')}</button>
            <button className="btn btn-sm" onClick={() => setOpen(false)}>{tr('Cancel')}</button>
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
    if (!confirm(tr('Make {name} the owner of “{name2}”? They become a space admin; your own role stays as it is.', { name: u.name, name2: space.name }))) return
    try {
      await api.put(`/spaces/${space.key}/owner`, { userId: u.id })
      qc.invalidateQueries({ queryKey: ['space'] })
      setPicking(false)
      setQ('')
      toast(tr('{name} owns this space now', { name: u.name }))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not change the owner'))
    }
  }
  return (
    <div className="card" style={{ marginTop: 28 }}>
      <b>{tr('Owner')}</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>{tr('The person responsible for this space and the first contact for questions about it. The owner is always a space admin.')}</p>
      <div className="row" style={{ gap: 8 }}>
        {space.ownerId && space.ownerName ? <Link to={`/people/${space.ownerId}`}>{space.ownerName}</Link> : <span className="faint">{tr('Nobody')}</span>}
        <span className="spacer" />
        {!picking && <button className="btn btn-sm" onClick={() => setPicking(true)}>{tr('Change owner…')}</button>}
      </div>
      {picking && (
        <div style={{ marginTop: 10 }}>
          <div className="row" style={{ gap: 6 }}>
            <input className="input" autoFocus placeholder={tr('Search people')} aria-label={tr('New owner')} value={q} onChange={(e) => setQ(e.target.value)} />
            <button className="btn btn-sm" onClick={() => { setPicking(false); setQ('') }}>{tr('Cancel')}</button>
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
  const perms = useQuery({
    queryKey: ['space-perms', space.key],
    queryFn: () => api.get<{ defaultRole: string; defaultDenied: Perm[]; anonymousAccess: boolean; grants: Grant[]; permissions: PermInfo[] }>(`/spaces/${space.key}/permissions`),
  })
  const users = useQuery({ queryKey: ['users-all'], queryFn: () => api.get<{ id: string; name: string }[]>('/users') })
  const groups = useQuery({ queryKey: ['groups'], queryFn: () => api.get<{ id: string; name: string }[]>('/groups') })
  const [grants, setGrants] = useState<Grant[]>([])
  const [defaultRole, setDefaultRole] = useState('editor')
  const [defaultDenied, setDefaultDenied] = useState<Perm[]>([])
  const [expanded, setExpanded] = useState<string | null>(null)
  const [anon, setAnon] = useState(false)
  const [pick, setPick] = useState('')
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (perms.data) {
      setGrants(perms.data.grants)
      setDefaultRole(perms.data.defaultRole)
      setDefaultDenied(perms.data.defaultDenied ?? [])
      setAnon(perms.data.anonymousAccess)
    }
  }, [perms.data])
  const save = async () => {
    setError(null)
    try {
      await api.put(`/spaces/${space.key}/permissions`, { defaultRole, defaultDenied, anonymousAccess: anon, grants })
      qc.invalidateQueries()
      toast(tr('Permissions saved'))
    } catch (e) {
      setError(e instanceof Error ? e.message : tr('Failed'))
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
  const catalog = perms.data?.permissions ?? []
  const toggle = (id: string) => setExpanded(expanded === id ? null : id)
  return (
    <div style={{ maxWidth: 720 }}>
      <div className="field">
        <label htmlFor="default-role">{tr('All workspace members (except guests)')}</label>
        <div className="row" style={{ gap: 12 }}>
          <select id="default-role" className="select" value={defaultRole} onChange={(e) => setDefaultRole(e.target.value)} style={{ maxWidth: 320 }}>
            <option value="none">{tr('No access')}</option>
            {ROLES.map((r) => <option key={r} value={r}>{spaceRoleName(r)}</option>)}
          </select>
          {defaultRole !== 'none' && <PermissionSummary role={defaultRole} denied={defaultDenied} catalog={catalog} open={expanded === 'default'} onToggle={() => toggle('default')} />}
        </div>
        {expanded === 'default' && defaultRole !== 'admin' && defaultRole !== 'none' && (
          <PermissionChecks role={defaultRole} denied={defaultDenied} catalog={catalog} onChange={setDefaultDenied} label={tr('all workspace members')} />
        )}
      </div>
      <label className="row" style={{ marginBottom: 18 }}>
        <input type="checkbox" checked={anon} onChange={(e) => setAnon(e.target.checked)} />{' '}{tr('Anyone on the internet can view this space (anonymous access)')}</label>
      <h3>{tr('People and groups')}</h3>
      <div className="row" style={{ marginBottom: 10 }}>
        <select className="select" value={pick} onChange={(e) => setPick(e.target.value)} style={{ maxWidth: 360 }}>
          <option value="">{tr('Add a person or group…')}</option>
          {groups.data?.map((g) => <option key={g.id} value={`group:${g.id}`}>{tr('{name} (group)', { name: g.name })}</option>)}
          {users.data?.map((u) => <option key={u.id} value={`user:${u.id}`}>{u.name}</option>)}
        </select>
        <button className="btn" onClick={add} disabled={!pick}>{tr('Add')}</button>
      </div>
      <table className="list">
        <tbody>
          {grants.map((g, i) => {
            const id = g.subjectType + g.subjectId
            const name = g.subjectName ?? g.subjectId
            return (
              <Fragment key={id}>
                <tr>
                  <td className="row">{g.subjectType === 'group' && <Users size={15} />} {name}</td>
                  <td style={{ width: 170 }}>
                    <select className="select" aria-label={tr('Role of {name}', { name })} value={g.role} onChange={(e) => setGrants(grants.map((x, j) => (j === i ? { ...x, role: e.target.value } : x)))}>
                      {ROLES.map((r) => <option key={r} value={r}>{spaceRoleName(r)}</option>)}
                    </select>
                  </td>
                  <td style={{ width: 170 }}>
                    <PermissionSummary role={g.role} denied={g.denied ?? []} catalog={catalog} open={expanded === id} onToggle={() => toggle(id)} />
                  </td>
                  <td style={{ width: 40 }}>
                    <button className="icon-btn" aria-label={tr('Remove {name}', { name })} onClick={() => setGrants(grants.filter((_, j) => j !== i))}><X size={14} /></button>
                  </td>
                </tr>
                {expanded === id && g.role !== 'admin' && (
                  <tr>
                    <td colSpan={4}>
                      <PermissionChecks role={g.role} denied={g.denied ?? []} catalog={catalog} label={name} onChange={(denied) => setGrants(grants.map((x, j) => (j === i ? { ...x, denied } : x)))} />
                    </td>
                  </tr>
                )}
              </Fragment>
            )
          })}
        </tbody>
      </table>
      {error && <p className="error-text">{error}</p>}
      <button className="btn btn-primary" style={{ marginTop: 16 }} onClick={save}>{tr('Save permissions')}</button>
    </div>
  )
}

/** Broken links in the space's pages, and the pages no other page links to. */
function LinkReportPanel({ space }: { space: Space }) {
  const report = useQuery({ queryKey: ['link-report', space.key], queryFn: () => api.get<LinkReport>(`/spaces/${space.key}/link-report`) })
  if (report.isLoading) return <Spinner />
  if (report.error) return <ErrorBox error={report.error} />
  const r = report.data!
  const problem = (b: LinkReport['brokenLinks'][number]) => {
    switch (b.problem) {
      case 'trashed':
        return tr('The page is in the trash')
      case 'deleted':
        return tr('The page was deleted')
      case 'missing':
        return (
          <Link to={`/missing?space=${encodeURIComponent(b.missingSpace ?? '')}&title=${encodeURIComponent(b.missingTitle ?? '')}`}>
            {tr('There is no page “{title}” in {space}', { title: b.missingTitle ?? '', space: b.missingSpace ?? '' })}
          </Link>
        )
      default:
        return tr('The page was not part of any import ({source})', { source: b.missingSource ?? '' })
    }
  }
  return (
    <div>
      {r.pending > 0 && <p className="muted small" style={{ marginTop: 0 }}>{tr('Still reading the links of {count} pages, so this report may be incomplete.', { count: r.pending })}</p>}
      <h3 className="row" style={{ gap: 8 }}><Unlink size={16} />{tr('Broken links')}{r.brokenLinks.length > 0 && <span className="faint small">{r.brokenLinks.length}</span>}</h3>
      <p className="muted" style={{ marginTop: 0 }}>{tr('Links in this space that lead to a page in the trash, a deleted page or a page that doesn’t exist.')}</p>
      {r.brokenLinks.length === 0 ? (
        <div className="empty">{tr('No broken links.')}</div>
      ) : (
        <table className="list">
          <tbody>
            {r.brokenLinks.map((b, i) => (
              <tr key={`${b.pageId}-${i}`}>
                <td><Link to={`/pages/${b.pageId}${b.blockId ? `#block-${b.blockId}` : ''}`}><b>{b.pageTitle || tr('Untitled')}</b></Link></td>
                <td>{b.via === 'include' ? tr('Included page') : b.via === 'property' ? tr('Property “{name}”', { name: b.label }) : `“${b.label}”`}</td>
                <td className="faint small">{problem(b)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h3 className="row" style={{ gap: 8, marginTop: 32 }}><Link2 size={16} />{tr('Orphaned pages')}{r.orphans.length > 0 && <span className="faint small">{r.orphans.length}</span>}</h3>
      <p className="muted" style={{ marginTop: 0 }}>{tr('Pages that no other page links to, includes or lists among its child pages. Readers find them only through the page tree or search.')}</p>
      {r.orphans.length === 0 ? (
        <div className="empty">{tr('No orphaned pages.')}</div>
      ) : (
        <table className="list">
          <tbody>
            {r.orphans.map((o) => (
              <tr key={o.id}>
                <td><Link to={`/pages/${o.id}`}><b>{o.title || tr('Untitled')}</b></Link></td>
                <td className="faint small">{o.parentId ? (o.parentTitle ? tr('Under {title}', { title: o.parentTitle }) : tr('Under a page')) : tr('Top level')}</td>
                <td className="faint small">{tr('Updated {date}', { date: formatDate(o.updatedAt) })}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {r.truncated && <p className="faint small">{tr('Only the first 1,000 of each are listed.')}</p>}
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
      <p className="muted" style={{ marginTop: 0 }}>{tr('Archived pages are read-only and hidden from the page tree and search. Their links keep working.')}</p>
      {list.data?.length === 0 && <div className="empty"><Archive size={28} /><br />{tr('No archived pages.')}</div>}
      <table className="list">
        <tbody>
          {list.data?.map((t) => (
            <tr key={t.id}>
              <td><Link to={`/pages/${t.id}`}><b>{t.title}</b></Link>{t.descendants > 0 && <span className="faint small">{' '}{tr('and {count} pages under it', { count: t.descendants })}</span>}</td>
              <td className="faint small">{tr('Archived {date} by', { date: formatDate(t.archivedAt) })}{' '}{t.archivedBy ?? tr('someone')}</td>
              <td style={{ width: 120 }}>
                {t.canRestore && (
                  <button
                    className="btn btn-sm"
                    onClick={async () => {
                      await api.post(`/pages/${t.id}/unarchive`)
                      qc.invalidateQueries({ queryKey: ['archived', space.key] })
                      qc.invalidateQueries({ queryKey: ['tree'] })
                      qc.invalidateQueries({ queryKey: ['page', t.id] })
                      toast(tr('Restored from the archive'))
                    }}
                  >
                    <RotateCcw size={13} />{' '}{tr('Restore')}</button>
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
    if (!confirm(tr('Delete “{title}” for good? Its history, comments, attachments and the trashed pages below it go too. This can\'t be undone.', { title }))) return
    try {
      const r = await api.post<{ purged: number }>(`/pages/${id}/purge`)
      refresh()
      toast(r.purged > 1 ? tr('Deleted {purged} pages for good', { purged: r.purged }) : tr('Deleted for good'))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not delete'))
    }
  }
  const empty = async () => {
    if (!confirm(tr('Delete everything in the trash of {name} for good? This can\'t be undone.', { name: space.name }))) return
    try {
      const r = await api.post<{ purged: number; kept: number }>(`/spaces/${space.key}/trash/empty`)
      refresh()
      toast(r.kept ? tr('Deleted {count} pages for good · {kept} restricted ones you can’t see stay', { count: r.purged, kept: r.kept }) : tr('Deleted {count} pages for good', { count: r.purged }))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not empty the trash'))
    }
  }
  return (
    <div>
      {isAdmin && !!trash.data?.length && (
        <div className="row" style={{ marginBottom: 12 }}>
          <span className="faint small">{tr('Restore pages, or delete them for good.')}</span>
          <span className="spacer" />
          <button className="btn btn-danger" onClick={empty}><Trash2 size={14} />{' '}{tr('Empty trash')}</button>
        </div>
      )}
      {trash.data?.length === 0 && <div className="empty"><Trash2 size={28} /><br />{tr('Trash is empty.')}</div>}
      <table className="list">
        <tbody>
          {trash.data?.map((t) => (
            <tr key={t.id}>
              <td><b>{t.title}</b></td>
              <td className="faint small">{tr('Deleted {date} by', { date: formatDate(t.deletedAt) })}{' '}{t.deletedByName ?? tr('someone')}</td>
              <td style={{ width: isAdmin ? 250 : 120 }}>
                <div className="row" style={{ gap: 6, justifyContent: 'flex-end' }}>
                  <button
                    className="btn btn-sm"
                    onClick={async () => {
                      await api.post(`/pages/${t.id}/restore`)
                      qc.invalidateQueries({ queryKey: ['trash', space.key] })
                      qc.invalidateQueries({ queryKey: ['tree', space.key] })
                      toast(tr('Restored'))
                    }}
                  >
                    <RotateCcw size={13} />{' '}{tr('Restore')}</button>
                  {isAdmin && (
                    <button className="btn btn-sm btn-danger" onClick={() => purge(t.id, t.title)}>{tr('Delete forever')}</button>
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
