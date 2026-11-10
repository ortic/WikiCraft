import { useEffect, useState } from 'react'
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom'
import { useCanonicalSpaceKey } from '../useCanonicalKey'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Archive, Trash2, RotateCcw, Users, X } from 'lucide-react'
import { api, formatDate, type Space } from '../api'
import { SymbolPicker } from '../icons'
import ExportPanel from '../components/ExportPanel'
import { ErrorBox, SpaceIcon, Spinner, useFeatures, useToast } from '../ui'

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
  return (
    <div>
      {trash.data?.length === 0 && <div className="empty"><Trash2 size={28} /><br />Trash is empty.</div>}
      <table className="list">
        <tbody>
          {trash.data?.map((t) => (
            <tr key={t.id}>
              <td><b>{t.title}</b></td>
              <td className="faint small">Deleted {formatDate(t.deletedAt)} by {t.deletedByName ?? 'someone'}</td>
              <td style={{ width: 120 }}>
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
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
