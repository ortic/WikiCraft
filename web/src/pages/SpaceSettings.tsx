import { useEffect, useState } from 'react'
import { useParams, useSearchParams } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Trash2, RotateCcw, Users, X } from 'lucide-react'
import { api, formatDate, type Space } from '../api'
import { ErrorBox, Spinner, useToast } from '../ui'

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
  const tab = params.get('tab') ?? 'details'
  const space = useQuery({ queryKey: ['space', key], queryFn: () => api.get<Space>(`/spaces/${key}`) })
  if (space.isLoading) return <div className="content-pad"><Spinner /></div>
  if (space.error) return <div className="content-pad"><ErrorBox error={space.error} /></div>
  const s = space.data!
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
        <button className={`tab ${tab === 'trash' ? 'active' : ''}`} onClick={() => setParams({ tab: 'trash' })}>Trash</button>
      </div>
      {tab === 'details' && s.role === 'admin' && <Details space={s} />}
      {tab === 'permissions' && s.role === 'admin' && <Permissions space={s} />}
      {tab === 'trash' && <TrashList space={s} />}
    </div>
  )
}

function Details({ space }: { space: Space }) {
  const qc = useQueryClient()
  const toast = useToast()
  const [name, setName] = useState(space.name)
  const [description, setDescription] = useState(space.description ?? '')
  const [icon, setIcon] = useState(space.icon ?? '')
  const save = async () => {
    await api.patch(`/spaces/${space.key}`, { name, description, icon })
    qc.invalidateQueries({ queryKey: ['spaces'] })
    qc.invalidateQueries({ queryKey: ['space', space.key] })
    toast('Saved')
  }
  return (
    <div style={{ maxWidth: 520 }}>
      <div className="field"><label>Name</label><input className="input" value={name} onChange={(e) => setName(e.target.value)} /></div>
      <div className="field"><label>Icon (emoji)</label><input className="input" value={icon} onChange={(e) => setIcon(e.target.value)} placeholder="📘" /></div>
      <div className="field"><label>Description</label><textarea className="textarea" value={description} onChange={(e) => setDescription(e.target.value)} /></div>
      <div className="field"><label>Key</label><input className="input" value={space.key} disabled /></div>
      <button className="btn btn-primary" onClick={save}>Save</button>
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
          {groups.data?.map((g) => <option key={g.id} value={`group:${g.id}`}>👥 {g.name}</option>)}
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
