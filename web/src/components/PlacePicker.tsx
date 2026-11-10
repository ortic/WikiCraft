import { useQuery } from '@tanstack/react-query'
import { api, type Space, type TreeNode } from '../api'

/** Where a page goes: a space you can write in and a parent page in it (or its top level).
 * `exclude` leaves out a page and everything below it (a page can't move below itself). */
export default function PlacePicker({
  spaceKey,
  parentId,
  onChange,
  exclude,
  idPrefix,
}: {
  spaceKey: string
  parentId: string | null
  onChange: (spaceKey: string, parentId: string | null) => void
  exclude?: string
  idPrefix: string
}) {
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<Space[]>('/spaces') })
  const tree = useQuery({ queryKey: ['tree', spaceKey], queryFn: () => api.get<{ nodes: TreeNode[] }>(`/spaces/${spaceKey}/tree`) })
  const writable = (spaces.data ?? []).filter((s) => (s.role === 'editor' || s.role === 'admin' || s.key === spaceKey) && !s.archivedAt)
  // the page tree, flattened in order with its depth (folders and pages; not links)
  const options: { id: string; title: string; depth: number }[] = []
  const nodes = (tree.data?.nodes ?? []).filter((n) => n.kind !== 'link' && n.kind !== 'blog')
  const byParent = new Map<string, TreeNode[]>()
  for (const n of nodes) byParent.set(n.parentId ?? '', [...(byParent.get(n.parentId ?? '') ?? []), n])
  const walk = (parent: string, depth: number) => {
    for (const n of (byParent.get(parent) ?? []).sort((a, b) => a.position - b.position)) {
      if (n.id === exclude || depth > 30) continue
      options.push({ id: n.id, title: n.title, depth })
      walk(n.id, depth + 1)
    }
  }
  walk('', 0)
  return (
    <>
      <div className="field">
        <label htmlFor={`${idPrefix}-space`}>Space</label>
        <select id={`${idPrefix}-space`} className="select" value={spaceKey} onChange={(e) => onChange(e.target.value, null)}>
          {writable.map((s) => (
            <option key={s.key} value={s.key}>
              {s.name} ({s.key})
            </option>
          ))}
        </select>
      </div>
      <div className="field">
        <label htmlFor={`${idPrefix}-parent`}>Below</label>
        <select id={`${idPrefix}-parent`} className="select" value={parentId ?? ''} onChange={(e) => onChange(spaceKey, e.target.value || null)}>
          <option value="">Top level of the space</option>
          {options.map((o) => (
            <option key={o.id} value={o.id}>
              {'  '.repeat(o.depth)}
              {o.title}
            </option>
          ))}
        </select>
      </div>
    </>
  )
}
