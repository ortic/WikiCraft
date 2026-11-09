import { useEffect, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Lock, Users, X } from 'lucide-react'
import { api, type Page } from '../api'
import AccessCheck from './AccessCheck'
import { Modal, useMe } from '../ui'

interface Entry {
  kind: 'view' | 'edit'
  subjectType: 'user' | 'group'
  subjectId: string
  subjectName?: string | null
}

interface Subject {
  id: string
  name: string
  type: 'user' | 'group'
}

export default function RestrictionsDialog({ page, onClose }: { page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const me = useMe()
  const data = useQuery({ queryKey: ['restrictions', page.id], queryFn: () => api.get<{ restrictions: Entry[]; inheritedFrom: { id: string; title: string }[] }>(`/pages/${page.id}/restrictions`) })
  const users = useQuery({ queryKey: ['users-all'], queryFn: () => api.get<{ id: string; name: string }[]>('/users') })
  const groups = useQuery({ queryKey: ['groups'], queryFn: () => api.get<{ id: string; name: string }[]>('/groups') })
  const [entries, setEntries] = useState<Entry[]>([])
  const [mode, setMode] = useState<'none' | 'edit' | 'view'>('none')
  const [pick, setPick] = useState('')
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!data.data) return
    const e = data.data.restrictions
    setEntries(e)
    setMode(e.some((x) => x.kind === 'view') ? 'view' : e.some((x) => x.kind === 'edit') ? 'edit' : 'none')
  }, [data.data])

  const subjects: Subject[] = [...(users.data ?? []).map((u) => ({ id: u.id, name: u.name, type: 'user' as const })), ...(groups.data ?? []).map((g) => ({ id: g.id, name: g.name, type: 'group' as const }))]
  const people = Array.from(new Map(entries.map((e) => [e.subjectType + e.subjectId, e])).values())
  const nameOf = (e: Entry) => e.subjectName ?? subjects.find((s) => s.id === e.subjectId)?.name ?? e.subjectId

  const changeMode = (m: typeof mode) => {
    setMode(m)
    const myId = me.data?.user?.id
    if (m !== 'none' && people.length === 0 && myId) {
      setEntries([{ kind: m, subjectType: 'user', subjectId: myId, subjectName: me.data?.user?.name }])
    }
  }
  const add = () => {
    const s = subjects.find((x) => x.type + ':' + x.id === pick)
    if (!s) return
    setEntries((es) => [...es.filter((e) => !(e.subjectId === s.id)), { kind: 'view', subjectType: s.type, subjectId: s.id, subjectName: s.name }])
    setPick('')
  }
  const save = async () => {
    setError(null)
    const kinds: ('view' | 'edit')[] = mode === 'none' ? [] : mode === 'view' ? ['view', 'edit'] : ['edit']
    const out = people.flatMap((p) => kinds.map((k) => ({ kind: k, subjectType: p.subjectType, subjectId: p.subjectId })))
    try {
      await api.put(`/pages/${page.id}/restrictions`, { restrictions: out })
      qc.invalidateQueries({ queryKey: ['page', page.id] })
      qc.invalidateQueries({ queryKey: ['tree', page.spaceKey] })
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed')
    }
  }
  return (
    <Modal
      title={
        <span className="row">
          <Lock size={16} /> Restrictions
        </span>
      }
      onClose={onClose}
      footer={
        page.access.admin ? (
          <>
            <button className="btn" onClick={onClose}>
              Cancel
            </button>
            <button className="btn btn-primary" onClick={save}>
              Apply
            </button>
          </>
        ) : undefined
      }
    >
      {data.data?.inheritedFrom.length ? (
        <div className="card small" style={{ marginBottom: 12 }}>
          Also restricted through: {data.data.inheritedFrom.map((p) => p.title).join(', ')} (view restrictions are inherited).
        </div>
      ) : null}
      <div className="field">
        <label>Who can see and edit this page?</label>
        <select className="select" value={mode} disabled={!page.access.admin} onChange={(e) => changeMode(e.target.value as typeof mode)}>
          <option value="none">Anyone with space access can view and edit</option>
          <option value="edit">Anyone can view, only specific people can edit</option>
          <option value="view">Only specific people can view and edit</option>
        </select>
      </div>
      {mode !== 'none' && (
        <>
          {page.access.admin && (
            <div className="row" style={{ marginBottom: 10 }}>
              <select className="select" value={pick} onChange={(e) => setPick(e.target.value)}>
                <option value="">Add a person or group…</option>
                {subjects.map((s) => (
                  <option key={s.type + s.id} value={s.type + ':' + s.id}>
                    {s.name}
                    {s.type === 'group' ? ' (group)' : ''}
                  </option>
                ))}
              </select>
              <button className="btn" onClick={add} disabled={!pick}>
                Add
              </button>
            </div>
          )}
          {people.map((p) => (
            <div key={p.subjectType + p.subjectId} className="row" style={{ padding: '6px 0', borderBottom: '1px solid var(--border)' }}>
              {p.subjectType === 'group' ? <Users size={15} /> : null}
              <span>{nameOf(p)}</span>
              <span className="spacer" />
              <span className="faint small">{mode === 'view' ? 'Can view & edit' : 'Can edit'}</span>
              {page.access.admin && (
                <button className="icon-btn" onClick={() => setEntries((es) => es.filter((e) => e.subjectId !== p.subjectId))}>
                  <X size={14} />
                </button>
              )}
            </div>
          ))}
        </>
      )}
      {error && <p className="error-text">{error}</p>}
      {page.access.admin && <AccessCheck pageId={page.id} people={users.data ?? []} />}
    </Modal>
  )
}
