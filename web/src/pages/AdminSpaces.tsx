import { Link } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Archive, ArchiveRestore, Trash2 } from 'lucide-react'
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
      toast(e instanceof Error ? e.message : 'Failed')
    }
  }
  if (spaces.isLoading) return <Spinner />
  return (
    <div>
      <p className="muted" style={{ marginTop: 0 }}>Archived spaces are read-only and hidden from search and the space list. Only archived spaces can be deleted, and deleting is permanent.</p>
      <table className="list">
        <thead>
          <tr><th>Space</th><th>Owner</th><th>Pages</th><th>Attachments</th><th>Last updated</th><th>Access</th><th /></tr>
        </thead>
        <tbody>
          {spaces.data?.map((s) => (
            <tr key={s.id} className={s.archivedAt ? 'archived-row' : undefined}>
              <td>
                <span className="row">
                  <SpaceIcon space={s} size={22} />
                  <Link to={`/spaces/${s.key}`}>{s.name}</Link>
                  <span className="faint small">{s.key}</span>
                  {s.archivedAt && <span className="badge">archived {formatDate(s.archivedAt)}</span>}
                  {s.personal && <span className="badge">personal</span>}
                </span>
              </td>
              <td className="small">
                {s.ownerId && s.ownerName ? <Link to={`/people/${s.ownerId}`}>{s.ownerName}</Link> : <span className="faint">nobody</span>}
                {!s.personal && !s.archivedAt && (!s.ownerId || !s.ownerActive) && (
                  <> <Link className="badge" to={`/spaces/${s.key}/settings?tab=details`} title="The owner has no active account: pick a new one in the space settings">needs an owner</Link></>
                )}
              </td>
              <td>{s.pages}</td>
              <td className="small">{s.attachments} · {formatBytes(s.storageBytes)}</td>
              <td className="faint small">{formatDate(s.lastUpdated)}</td>
              <td className="small">{s.anonymousAccess ? 'public' : s.defaultRole === 'none' ? 'invited only' : `members: ${s.defaultRole}`}</td>
              <td style={{ textAlign: 'right', whiteSpace: 'nowrap' }}>
                {s.archivedAt ? (
                  <>
                    <button className="btn small" onClick={() => act(() => api.post(`/admin/spaces/${s.key}/unarchive`), `${s.name} restored`)}><ArchiveRestore size={13} /> Restore</button>{' '}
                    <button
                      className="btn btn-ghost small"
                      title="Delete permanently"
                      onClick={() => {
                        const typed = window.prompt(`This permanently deletes “${s.name}” with ${s.pages} pages and ${s.attachments} attachments. Type the space key (${s.key}) to confirm.`)
                        if (typed) act(() => api.del(`/admin/spaces/${s.key}`, { confirm: typed }), `${s.name} deleted`)
                      }}
                    >
                      <Trash2 size={13} />
                    </button>
                  </>
                ) : (
                  <button className="btn small" onClick={() => confirm(`Archive “${s.name}”? It becomes read-only and leaves search.`) && act(() => api.del(`/spaces/${s.key}`), `${s.name} archived`)}>
                    <Archive size={13} /> Archive
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
