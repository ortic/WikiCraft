import { t } from '../i18n'
import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Archive, ArchiveRestore, Trash2, Users, X } from 'lucide-react'
import { api, formatBytes, formatDate } from '../api'
import { SpaceIcon, Spinner, useToast } from '../ui'

interface SpaceRow {
  id: string
  key: string
  name: string
  icon: string | null
  defaultRole: string
  anonymousAccess: boolean
  archivedAt: string | null
  pages: number
  attachments: number
  storageBytes: number
  lastUpdated: string | null
  personal: boolean
  ownerId: string | null
  ownerName: string | null
  /** the owner's account is active */
  ownerActive: boolean
}

/** Admin → Spaces: every space incl. archived ones; archive, restore, delete. */
export default function AdminSpaces() {
  const qc = useQueryClient()
  const toast = useToast()
  const spaces = useQuery({ queryKey: ['admin-spaces'], queryFn: () => api.get<SpaceRow[]>('/admin/spaces') })
  const refresh = () => {
    qc.invalidateQueries({ queryKey: ['admin-spaces'] })
    qc.invalidateQueries({ queryKey: ['spaces'] })
  }
  const act = async (fn: () => Promise<unknown>, msg: string) => {
    try {
      await fn()
      refresh()
      toast(msg)
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Failed'))
    }
  }
  if (spaces.isLoading) return <Spinner />
  return (
    <div>
      <NewSpaceDefaults />
      <p className="muted" style={{ marginTop: 0 }}>{t('Archived spaces are read-only and hidden from search and the space list. Only archived spaces can be deleted, and deleting is permanent.')}</p>
      <table className="list">
        <thead>
          <tr><th>{t('Space')}</th><th>{t('Owner')}</th><th>{t('Pages')}</th><th>{t('Attachments')}</th><th>{t('Last updated')}</th><th>{t('Access')}</th><th /></tr>
        </thead>
        <tbody>
          {spaces.data?.map((s) => (
            <tr key={s.id} className={s.archivedAt ? 'archived-row' : undefined}>
              <td>
                <span className="row">
                  <SpaceIcon space={s} size={22} />
                  <Link to={`/spaces/${s.key}`}>{s.name}</Link>
                  <span className="faint small">{s.key}</span>
                  {s.archivedAt && <span className="badge">{t('archived {date}', { date: formatDate(s.archivedAt) })}</span>}
                  {s.personal && <span className="badge">{t('personal')}</span>}
                </span>
              </td>
              <td className="small">
                {s.ownerId && s.ownerName ? <Link to={`/people/${s.ownerId}`}>{s.ownerName}</Link> : <span className="faint">{t('nobody')}</span>}
                {!s.personal && !s.archivedAt && (!s.ownerId || !s.ownerActive) && (
                  <> <Link className="badge" to={`/spaces/${s.key}/settings?tab=details`} title={t('The owner has no active account: pick a new one in the space settings')}>{t('needs an owner')}</Link></>
                )}
              </td>
              <td>{s.pages}</td>
              <td className="small">{s.attachments} · {formatBytes(s.storageBytes)}</td>
              <td className="faint small">{formatDate(s.lastUpdated)}</td>
              <td className="small">{s.anonymousAccess ? t('public') : s.defaultRole === 'none' ? t('invited only') : t('members: {defaultRole}', { defaultRole: s.defaultRole })}</td>
              <td style={{ textAlign: 'right', whiteSpace: 'nowrap' }}>
                {s.archivedAt ? (
                  <>
                    <button className="btn small" onClick={() => act(() => api.post(`/admin/spaces/${s.key}/unarchive`), `${s.name} restored`)}><ArchiveRestore size={13} />{' '}{t('Restore')}</button>{' '}
                    <button
                      className="btn btn-ghost small"
                      title={t('Delete permanently')}
                      onClick={() => {
                        const typed = window.prompt(t('This permanently deletes “{name}” with {pages} pages and {attachments} attachments. Type the space key ({key}) to confirm.', { name: s.name, pages: s.pages, attachments: s.attachments, key: s.key }))
                        if (typed) act(() => api.del(`/admin/spaces/${s.key}`, { confirm: typed }), `${s.name} deleted`)
                      }}
                    >
                      <Trash2 size={13} />
                    </button>
                  </>
                ) : (
                  <button className="btn small" onClick={() => confirm(t('Archive “{name}”? It becomes read-only and leaves search.', { name: s.name })) && act(() => api.del(`/spaces/${s.key}`), `${s.name} archived`)}>
                    <Archive size={13} />{' '}{t('Archive')}</button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

interface DefaultGrant {
  subjectType: 'user' | 'group'
  subjectId: string
  role: string
}

const ROLES = ['viewer', 'commenter', 'editor', 'admin']

/** Admin → Spaces → New spaces: the role everyone gets in a new space (unless its creator picks
 *  another) and the people and groups added to every new space. */
function NewSpaceDefaults() {
  const qc = useQueryClient()
  const toast = useToast()
  const settings = useQuery({ queryKey: ['admin-settings'], queryFn: () => api.get<{ newSpaces: { defaultRole: string; grants: DefaultGrant[] } }>('/admin/settings') })
  const users = useQuery({ queryKey: ['users-all'], queryFn: () => api.get<{ id: string; name: string; status?: string }[]>('/users') })
  const groups = useQuery({ queryKey: ['groups'], queryFn: () => api.get<{ id: string; name: string }[]>('/groups') })
  const [role, setRole] = useState('editor')
  const [grants, setGrants] = useState<DefaultGrant[]>([])
  const [pick, setPick] = useState('')
  const [dirty, setDirty] = useState(false)
  useEffect(() => {
    if (settings.data?.newSpaces && !dirty) {
      setRole(settings.data.newSpaces.defaultRole)
      setGrants(settings.data.newSpaces.grants)
    }
  }, [settings.data, dirty])
  const nameOf = (g: DefaultGrant) => (g.subjectType === 'group' ? groups.data?.find((x) => x.id === g.subjectId)?.name : users.data?.find((x) => x.id === g.subjectId)?.name) ?? t('Unknown')
  const save = async () => {
    try {
      await api.put('/admin/settings', { newSpaces: { defaultRole: role, grants } })
      setDirty(false)
      qc.invalidateQueries({ queryKey: ['admin-settings'] })
      qc.invalidateQueries({ queryKey: ['features'] })
      toast(t('Saved: new spaces start like this'))
    } catch (e) {
      toast(e instanceof Error ? e.message : t('Could not save'))
    }
  }
  const add = () => {
    const [type, id] = pick.split(':')
    if (!id || grants.some((g) => g.subjectId === id)) return
    setGrants([...grants, { subjectType: type as 'user' | 'group', subjectId: id, role: 'admin' }])
    setPick('')
    setDirty(true)
  }
  if (!settings.data) return null
  return (
    <div className="card" style={{ maxWidth: 720, marginBottom: 20 }}>
      <h3 style={{ marginTop: 0 }}>{t('New spaces')}</h3>
      <p className="muted small" style={{ marginTop: 0 }}>{t('How spaces start out. Whoever creates a space becomes its admin and can change all of this afterwards.')}</p>
      <div className="field">
        <label htmlFor="ns-role">{t('Everyone signed in (except guests)')}</label>
        <select id="ns-role" className="select" style={{ maxWidth: 260 }} value={role} onChange={(e) => { setRole(e.target.value); setDirty(true) }}>
          <option value="none">{t('No access')}</option>
          {ROLES.filter((r) => r !== 'admin').map((r) => <option key={r} value={r}>{r}</option>)}
        </select>
        <p className="faint small" style={{ margin: '4px 0 0' }}>{t('Preselected when someone creates a space; they can still pick another.')}</p>
      </div>
      <div className="field" style={{ marginBottom: 8 }}>
        <label htmlFor="ns-pick">{t('Added to every new space')}</label>
        <div className="row" style={{ gap: 6 }}>
          <select id="ns-pick" className="select" style={{ maxWidth: 320 }} value={pick} onChange={(e) => setPick(e.target.value)}>
            <option value="">{t('Add a person or group…')}</option>
            {groups.data?.map((g) => <option key={g.id} value={`group:${g.id}`}>{t('{name} (group)', { name: g.name })}</option>)}
            {users.data?.filter((u) => !u.status || u.status === 'active').map((u) => <option key={u.id} value={`user:${u.id}`}>{u.name}</option>)}
          </select>
          <button className="btn" disabled={!pick} onClick={add}>{t('Add')}</button>
        </div>
      </div>
      {grants.length > 0 && (
        <table className="list">
          <tbody>
            {grants.map((g, i) => (
              <tr key={g.subjectType + g.subjectId}>
                <td className="row">{g.subjectType === 'group' && <Users size={15} />} {nameOf(g)}</td>
                <td style={{ width: 170 }}>
                  <select className="select" aria-label={t('Role of {nameOf}', { nameOf: nameOf(g) })} value={g.role} onChange={(e) => { setGrants(grants.map((x, j) => (j === i ? { ...x, role: e.target.value } : x))); setDirty(true) }}>
                    {ROLES.map((r) => <option key={r} value={r}>{r}</option>)}
                  </select>
                </td>
                <td style={{ width: 40 }}>
                  <button className="icon-btn" aria-label={t('Remove {nameOf}', { nameOf: nameOf(g) })} onClick={() => { setGrants(grants.filter((_, j) => j !== i)); setDirty(true) }}><X size={14} /></button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <button className="btn btn-primary btn-sm" style={{ marginTop: 10 }} disabled={!dirty} onClick={save}>{t('Save')}</button>
    </div>
  )
}
