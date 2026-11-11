import { t } from '../i18n'
import { useMemo, useState, type DragEvent } from 'react'
import { LineIcon, parseSymbol } from '../icons'
import { useMatch, useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ChevronRight, ChevronDown, FileText, Folder, FolderPlus, Plus, Lock, Link2, ExternalLink } from 'lucide-react'
import { api, can, type Space, type TreeNode } from '../api'
import { useToast } from '../ui'

interface TreeData {
  spaceId: string
  homePageId: string | null
  nodes: TreeNode[]
}

type DropPos = 'before' | 'after' | 'inside'

export default function SpaceTree({ space }: { space: Space }) {
  const qc = useQueryClient()
  const nav = useNavigate()
  const toast = useToast()
  const active = useMatch('/pages/:id/*')?.params.id
  const tree = useQuery({ queryKey: ['tree', space.key], queryFn: () => api.get<TreeData>(`/spaces/${space.key}/tree`) })
  const [open, setOpen] = useState<Record<string, boolean>>({})
  const [drag, setDrag] = useState<string | null>(null)
  const [drop, setDrop] = useState<{ id: string; pos: DropPos } | null>(null)
  const canEdit = space.role === 'editor' || space.role === 'admin'
  const canAdd = canEdit && can(space, 'page.add')

  const { children, parentOf } = useMemo(() => {
    const children = new Map<string, TreeNode[]>()
    const parentOf = new Map<string, string | null>()
    const ids = new Set(tree.data?.nodes.map((n) => n.id))
    for (const n of tree.data?.nodes ?? []) {
      const p = n.parentId && ids.has(n.parentId) ? n.parentId : ''
      parentOf.set(n.id, p || null)
      children.set(p, [...(children.get(p) ?? []), n])
    }
    for (const list of children.values()) list.sort((a, b) => a.position - b.position || a.title.localeCompare(b.title))
    return { children, parentOf }
  }, [tree.data])

  // auto-expand ancestors of the active page
  const expanded = useMemo(() => {
    const e = { ...open }
    let cur = active ? parentOf.get(active) : null
    let guard = 0
    while (cur && guard++ < 200) {
      if (e[cur] === undefined) e[cur] = true
      cur = parentOf.get(cur) ?? null
    }
    return e
  }, [open, active, parentOf])

  const createChild = async (parentId: string | null, kind: 'page' | 'folder' = 'page') => {
    const r = await api.post<{ id: string }>('/pages', { spaceKey: space.key, parentId, kind, title: kind === 'folder' ? t('New folder') : t('Untitled') })
    if (parentId) setOpen((o) => ({ ...o, [parentId]: true }))
    await qc.invalidateQueries({ queryKey: ['tree', space.key] })
    nav(`/pages/${r.id}?new=1`)
  }

  const onDrop = async (e: DragEvent, target: TreeNode) => {
    e.preventDefault()
    const d = drop
    setDrop(null)
    if (!drag || !d || drag === target.id) return
    const body =
      d.pos === 'inside'
        ? { parentId: target.id }
        : { parentId: parentOf.get(target.id) ?? null, ...(d.pos === 'before' ? { beforeId: target.id } : { afterId: target.id }) }
    try {
      await api.post(`/pages/${drag}/move`, body)
      if (d.pos === 'inside') setOpen((o) => ({ ...o, [target.id]: true }))
      await qc.invalidateQueries({ queryKey: ['tree', space.key] })
      qc.invalidateQueries({ queryKey: ['page', drag] })
    } catch (err) {
      toast(err instanceof Error ? err.message : t('Move failed'))
    }
    setDrag(null)
  }

  const renderNodes = (parent: string, depth: number): React.ReactNode =>
    (children.get(parent) ?? []).map((n) => {
      const kids = children.get(n.id)
      const isOpen = !!expanded[n.id]
      const dropCls = drop?.id === n.id ? `drop-${drop.pos}` : ''
      return (
        <div key={n.id}>
          <div
            className={`tree-row ${active === n.id ? 'active' : ''} ${dropCls}`}
            style={{ paddingLeft: 4 + depth * 14 }}
            onClick={() => (n.kind === 'link' && n.url ? window.open(n.url, '_blank', 'noopener,noreferrer') : nav(`/pages/${n.id}`))}
            draggable={canEdit && n.id !== tree.data?.homePageId}
            onDragStart={(e) => {
              setDrag(n.id)
              e.dataTransfer.effectAllowed = 'move'
            }}
            onDragEnd={() => {
              setDrag(null)
              setDrop(null)
            }}
            onDragOver={(e) => {
              if (!drag || drag === n.id) return
              e.preventDefault()
              const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
              const y = e.clientY - r.top
              const pos: DropPos = y < r.height * 0.28 ? 'before' : y > r.height * 0.72 ? 'after' : 'inside'
              if (drop?.id !== n.id || drop.pos !== pos) setDrop({ id: n.id, pos })
            }}
            onDragLeave={() => setDrop((d) => (d?.id === n.id ? null : d))}
            onDrop={(e) => onDrop(e, n)}
          >
            <span
              className="caret"
              onClick={(e) => {
                e.stopPropagation()
                setOpen((o) => ({ ...o, [n.id]: !isOpen }))
              }}
            >
              {kids?.length ? isOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} /> : null}
            </span>
            {parseSymbol(n.icon) ? <LineIcon group="symbols" name={parseSymbol(n.icon)!.name} size={14} style={{ flex: 'none' }} /> : n.kind === 'folder' ? <Folder size={14} style={{ flex: 'none' }} /> : n.kind === 'link' ? <Link2 size={14} style={{ flex: 'none', opacity: 0.8 }} /> : <FileText size={14} style={{ flex: 'none', opacity: 0.7 }} />}
            <span className={`title${n.draft ? ' tree-draft' : ''}`} style={{ marginLeft: 6 }}>
              {n.title || t('Untitled')}
            </span>
            {n.draft && <span className="tree-draft-badge">{t('draft')}</span>}
            {n.kind === 'link' && <ExternalLink size={11} className="faint" style={{ flex: 'none' }} aria-label={t('opens in a new tab')} />}
            {n.restricted && <Lock size={12} className="faint" />}
            {canAdd && (
              <span className="actions">
                <button
                  className="icon-btn"
                  style={{ width: 22, height: 22 }}
                  title={t('Add child page')}
                  onClick={(e) => {
                    e.stopPropagation()
                    createChild(n.id)
                  }}
                >
                  <Plus size={13} />
                </button>
              </span>
            )}
          </div>
          {isOpen && kids && renderNodes(n.id, depth + 1)}
        </div>
      )
    })

  return (
    <div style={{ marginTop: 6 }}>
      <div className="sidebar-section" style={{ marginTop: 8 }}>{t('Pages')}{' '}<span className="spacer" />
        {canAdd && (
          <>
            <button className="icon-btn" style={{ width: 24, height: 24 }} title={t('New folder at top level')} aria-label={t('New folder at top level')} onClick={() => createChild(null, 'folder')}>
              <FolderPlus size={14} />
            </button>
            <button className="icon-btn" style={{ width: 24, height: 24 }} title={t('New page at top level')} aria-label={t('New page at top level')} onClick={() => createChild(null)}>
              <Plus size={14} />
            </button>
          </>
        )}
      </div>
      {tree.isLoading ? <div style={{ padding: 8 }} className="spinner" /> : renderNodes('', 0)}
    </div>
  )
}
