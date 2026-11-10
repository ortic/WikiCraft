// A folder: a container in the page tree without content of its own (Confluence Cloud folders
// import as these). Shows what's inside, and lets editors add pages and folders to it.

import { Link, useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { FilePlus2, FileText, Folder, FolderPlus, Link2 } from 'lucide-react'
import { api, formatDate, type Page } from '../api'
import { LineIcon, parseSymbol } from '../icons'
import { Spinner, useToast } from '../ui'

interface Child {
  id: string
  title: string
  kind: string
  icon: string | null
  parentId: string | null
  updatedAt: string
  excerpt: string
}

export function FolderView({ page, canEdit }: { page: Page; canEdit: boolean }) {
  const qc = useQueryClient()
  const nav = useNavigate()
  const toast = useToast()
  const children = useQuery({ queryKey: ['children', page.id, 1], queryFn: () => api.get<Child[]>(`/pages/${page.id}/children?depth=1`) })
  const create = async (kind: 'page' | 'folder') => {
    try {
      const r = await api.post<{ id: string }>('/pages', { spaceKey: page.spaceKey, parentId: page.id, kind, title: kind === 'folder' ? 'New folder' : 'Untitled' })
      qc.invalidateQueries({ queryKey: ['tree', page.spaceKey] })
      qc.invalidateQueries({ queryKey: ['children', page.id] })
      nav(`/pages/${r.id}?new=1`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not create it')
    }
  }
  const icon = (c: Child) => {
    const sym = parseSymbol(c.icon)
    if (sym) return <LineIcon group="symbols" name={sym.name} size={16} />
    if (c.kind === 'folder') return <Folder size={16} />
    if (c.kind === 'link') return <Link2 size={16} />
    return <FileText size={16} />
  }
  const rows = (children.data ?? []).filter((c) => c.parentId === page.id)
  return (
    <div className="folder-view">
      {canEdit && (
        <div className="row folder-actions">
          <button className="btn btn-sm" onClick={() => create('page')}><FilePlus2 size={14} /> New page</button>
          <button className="btn btn-sm" onClick={() => create('folder')}><FolderPlus size={14} /> New folder</button>
        </div>
      )}
      {children.isLoading ? (
        <Spinner />
      ) : rows.length === 0 ? (
        <div className="empty">
          <Folder size={28} />
          <br />
          This folder is empty.
        </div>
      ) : (
        <ul className="folder-list">
          {rows.map((c) => (
            <li key={c.id}>
              <Link to={`/pages/${c.id}`} className="folder-item">
                <span className="folder-item-icon">{icon(c)}</span>
                <span className="folder-item-text">
                  <span className="folder-item-title">{c.title || 'Untitled'}</span>
                  {c.excerpt && <span className="faint small folder-item-excerpt">{c.excerpt}</span>}
                </span>
                <span className="faint small">{formatDate(c.updatedAt)}</span>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}
