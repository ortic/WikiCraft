// Live rendering of Confluence's dynamic macros (imported as `macro` nodes): content by label,
// recently updated, page properties report, task report, attachments and page tree. Other
// macros keep the placeholder view.

import { useQuery } from '@tanstack/react-query'
import { Link } from 'react-router-dom'
import { CheckSquare, Clock, Download, FileText, ListTree, Milestone, Paperclip, Puzzle, Table2, Tag } from 'lucide-react'
import { api, formatBytes, formatDate, type Attachment, type Page } from '../api'

type Params = Record<string, string>

interface ReportPage {
  id: string
  title: string
  kind: string
  spaceKey: string
  spaceName: string
  updatedAt: string
  updatedByName: string | null
  labels: string[]
}

export const LIVE_MACROS = new Set(['contentbylabel', 'recently-updated', 'detailssummary', 'content-report-table', 'tasks-report-macro', 'decisionreport', 'attachments', 'pagetree'])

const csv = (s: string | undefined) => (s ?? '').split(',').map((x) => x.trim().replace(/^["']|["']$/g, '')).filter(Boolean)

/** Labels and spaces from classic parameters or a Cloud CQL query. */
export function querySpec(params: Params, currentSpace: string | undefined) {
  const cql = params.cql ?? ''
  const labels = new Set<string>([...csv(params.labels), ...csv(params.label)].map((l) => l.toLowerCase()))
  for (const m of cql.matchAll(/label\s*(?:=|in)\s*(?:\(([^)]*)\)|"([^"]*)"|'([^']*)'|([\w.-]+))/gi)) {
    csv(m[1] ?? m[2] ?? m[3] ?? m[4]).forEach((l) => labels.add(l.toLowerCase()))
  }
  let spaces = csv(params.spaces ?? params.space ?? params.spaceKey)
  for (const m of cql.matchAll(/space\s*(?:=|in)\s*(?:\(([^)]*)\)|"([^"]*)"|'([^']*)'|([\w.()-]+))/gi)) {
    spaces.push(...csv(m[1] ?? m[2] ?? m[3] ?? m[4]))
  }
  spaces = spaces.map((s) => (s === '@self' || /currentSpace\(\)/i.test(s) ? currentSpace ?? '' : s)).filter((s) => s && s !== '@all' && s !== '@global' && s !== '@personal')
  const all = (params.operator ?? '').toUpperCase() === 'AND' || /label\s*=.*\band\b.*label\s*=/i.test(cql)
  return { labels: [...labels], spaces: [...new Set(spaces.map((s) => s.toUpperCase()))], all }
}

function qs(o: Record<string, string | number | undefined>) {
  return Object.entries(o)
    .filter(([, v]) => v !== undefined && v !== '')
    .map(([k, v]) => `${k}=${encodeURIComponent(String(v))}`)
    .join('&')
}

function Header({ icon, title, detail }: { icon: React.ReactNode; title: string; detail?: string }) {
  return (
    <div className="block-label">
      {icon} {title}
      {detail && <span className="faint" style={{ textTransform: 'none', fontWeight: 400 }}> · {detail}</span>}
    </div>
  )
}

function Empty({ q, text }: { q: { isLoading: boolean; error: unknown }; text: string }) {
  if (q.isLoading) return <div className="spinner" />
  if (q.error) return <div className="faint small">Could not load: {q.error instanceof Error ? q.error.message : 'error'}</div>
  return <div className="faint small">{text}</div>
}

function PageList({ params, page, mode }: { params: Params; page: Page | undefined; mode: 'label' | 'recent' }) {
  const spec = querySpec(params, page?.spaceKey)
  // recently-updated defaults to the current space; contentbylabel to all spaces
  const spaces = spec.spaces.length ? spec.spaces : mode === 'recent' && !(params.spaces ?? '').includes('@all') && page ? [page.spaceKey] : []
  const max = Number(params.max ?? params.maxResults ?? (mode === 'recent' ? 15 : 20)) || 20
  const sort = mode === 'recent' ? 'updated' : params.sort === 'title' || !params.sort ? 'title' : params.sort === 'creation' ? 'created' : 'updated'
  const enabled = mode === 'recent' || spec.labels.length > 0
  const q = useQuery({
    queryKey: ['report-pages', mode, spec.labels, spaces, spec.all, sort, max],
    queryFn: () => api.get<ReportPage[]>(`/reports/pages?${qs({ labels: spec.labels.join(','), spaces: spaces.join(','), operator: spec.all ? 'all' : undefined, sort, max })}`),
    enabled,
  })
  let rows = (q.data ?? []).filter((r) => r.id !== page?.id || mode === 'recent')
  if (params.reverse === 'true') rows = [...rows].reverse()
  const showLabels = params.showLabels !== 'false' && mode === 'label'
  const showSpace = params.showSpace !== 'false' && spaces.length !== 1
  return (
    <>
      <Header
        icon={mode === 'recent' ? <Clock size={13} /> : <Tag size={13} />}
        title={mode === 'recent' ? 'Recently updated' : 'Content by label'}
        detail={[spec.labels.length ? `labels: ${spec.labels.join(spec.all ? ' and ' : ', ')}` : '', spaces.length ? `in ${spaces.join(', ')}` : ''].filter(Boolean).join(' ')}
      />
      {!enabled ? (
        <div className="faint small">No labels in this macro.</div>
      ) : rows.length === 0 ? (
        <Empty q={q} text="No pages found." />
      ) : (
        <ul className="macro-list">
          {rows.map((r) => (
            <li key={r.id}>
              <FileText size={14} className="faint" />
              <Link to={`/pages/${r.id}`}>{r.title}</Link>
              {showSpace && <span className="faint small">{r.spaceName}</span>}
              {mode === 'recent' && <span className="faint small">updated {formatDate(r.updatedAt)}{r.updatedByName ? ` by ${r.updatedByName}` : ''}</span>}
              {showLabels && r.labels.length > 0 && (
                <span className="macro-labels">
                  {r.labels.map((l) => (
                    <Link key={l} to={`/labels/${encodeURIComponent(l)}`} className="label-chip">{l}</Link>
                  ))}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </>
  )
}

function PropertiesReport({ params, page }: { params: Params; page: Page | undefined }) {
  const spec = querySpec(params, page?.spaceKey)
  const max = Number(params.pageSize ?? params.max ?? params.maxResults ?? 100) || 100
  const q = useQuery({
    queryKey: ['report-props', spec.labels, spec.spaces, max],
    queryFn: () => api.get<{ columns: string[]; rows: { page: ReportPage; properties: Record<string, string> }[] }>(`/reports/properties?${qs({ labels: spec.labels.join(','), spaces: spec.spaces.join(','), max, sort: 'title' })}`),
    enabled: spec.labels.length > 0,
  })
  const headings = csv(params.headings)
  const columns = headings.length ? headings : (q.data?.columns ?? []).slice(0, 8)
  const value = (props: Record<string, string>, col: string) => props[col] ?? Object.entries(props).find(([k]) => k.toLowerCase() === col.toLowerCase())?.[1] ?? ''
  const rows = q.data?.rows ?? []
  return (
    <>
      <Header icon={<Table2 size={13} />} title="Page properties report" detail={spec.labels.length ? `label: ${spec.labels.join(', ')}` : undefined} />
      {!spec.labels.length ? (
        <div className="faint small">No label in this report.</div>
      ) : rows.length === 0 ? (
        <Empty q={q} text="No pages with page properties and this label." />
      ) : (
        <div className="macro-table-wrap">
          <table className="macro-table">
            <thead>
              <tr>
                <th>{params.firstcolumn || 'Title'}</th>
                {columns.map((c) => (
                  <th key={c}>{c}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.page.id}>
                  <td><Link to={`/pages/${r.page.id}`}>{r.page.title}</Link></td>
                  {columns.map((c) => (
                    <td key={c}>{value(r.properties, c)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}

interface TaskRow {
  pageId: string
  idx: number
  text: string
  done: boolean
  due: string | null
  assigneeName: string | null
  pageTitle: string
}

function TasksReport({ params, page }: { params: Params; page: Page | undefined }) {
  const spec = querySpec(params, page?.spaceKey)
  const spaces = spec.spaces.length ? spec.spaces : page ? [page.spaceKey] : []
  const status = /^complete$/i.test(params.status ?? '') ? 'done' : /all/i.test(params.status ?? '') ? 'all' : 'open'
  const max = Number(params.pageSize ?? 20) || 20
  const q = useQuery({
    queryKey: ['report-tasks', spaces, spec.labels, status, max],
    queryFn: () => api.get<TaskRow[]>(`/reports/tasks?${qs({ spaces: spaces.join(','), labels: spec.labels.join(','), status, max })}`),
  })
  const rows = q.data ?? []
  return (
    <>
      <Header icon={<CheckSquare size={13} />} title="Task report" detail={[status === 'open' ? 'open tasks' : status === 'done' ? 'completed tasks' : 'all tasks', spaces.length ? `in ${spaces.join(', ')}` : '', spec.labels.length ? `labels: ${spec.labels.join(', ')}` : ''].filter(Boolean).join(' ')} />
      {rows.length === 0 ? (
        <Empty q={q} text="No tasks." />
      ) : (
        <div className="macro-table-wrap">
          <table className="macro-table">
            <thead>
              <tr><th style={{ width: 28 }} /><th>Task</th><th>Assignee</th><th>Due</th><th>Page</th></tr>
            </thead>
            <tbody>
              {rows.map((t) => (
                <tr key={`${t.pageId}-${t.idx}`}>
                  <td><input type="checkbox" checked={t.done} readOnly aria-label={t.done ? 'done' : 'open'} /></td>
                  <td>{t.text}</td>
                  <td>{t.assigneeName ?? <span className="faint">—</span>}</td>
                  <td>{t.due ?? ''}</td>
                  <td><Link to={`/pages/${t.pageId}`}>{t.pageTitle}</Link></td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}

interface DecisionRow {
  pageId: string
  pageTitle: string
  spaceKey: string
  text: string
  decided: boolean
  updatedAt: string
}

/** Decision log (`decisionreport`): the decisions recorded in a space's pages. */
function DecisionsReport({ params, page }: { params: Params; page: Page | undefined }) {
  const spec = querySpec(params, page?.spaceKey)
  const spaces = spec.spaces.length ? spec.spaces : page ? [page.spaceKey] : []
  const max = Number(params.max ?? params.maxResults ?? 30) || 30
  const q = useQuery({
    queryKey: ['report-decisions', spaces, spec.labels, max],
    queryFn: () => api.get<DecisionRow[]>(`/reports/decisions?${qs({ spaces: spaces.join(','), labels: spec.labels.join(','), max })}`),
    enabled: spaces.length > 0,
  })
  const rows = q.data ?? []
  return (
    <>
      <Header icon={<Milestone size={13} />} title="Decision log" detail={[spaces.length ? `in ${spaces.join(', ')}` : '', spec.labels.length ? `labels: ${spec.labels.join(', ')}` : ''].filter(Boolean).join(' ')} />
      {rows.length === 0 ? (
        <Empty q={q} text="No decisions recorded yet. Add one with /decision on any page." />
      ) : (
        <div className="macro-table-wrap">
          <table className="macro-table">
            <thead>
              <tr><th>Decision</th><th>Page</th><th>Updated</th></tr>
            </thead>
            <tbody>
              {rows.map((d, i) => (
                <tr key={`${d.pageId}-${i}`}>
                  <td>{d.text}{!d.decided && <span className="badge badge-warning" style={{ marginLeft: 6 }}>open</span>}</td>
                  <td><Link to={`/pages/${d.pageId}`}>{d.pageTitle}</Link></td>
                  <td className="faint">{formatDate(d.updatedAt)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}

function globToRegex(glob: string) {
  return new RegExp('^' + glob.replace(/[.+^${}()|[\]\\]/g, '\\$&').replace(/\*/g, '.*').replace(/\?/g, '.') + '$', 'i')
}

function AttachmentsList({ params, pageId }: { params: Params; pageId: string }) {
  const q = useQuery({ queryKey: ['attachments', pageId], queryFn: () => api.get<Attachment[]>(`/pages/${pageId}/attachments`) })
  const patterns = csv(params.patterns).map(globToRegex)
  let rows = (q.data ?? []).filter((a) => !patterns.length || patterns.some((r) => r.test(a.filename)))
  const by = params.sortBy ?? 'date'
  rows = [...rows].sort((a, b) => (by === 'name' ? a.filename.localeCompare(b.filename) : by === 'size' ? b.size - a.size : b.createdAt.localeCompare(a.createdAt)))
  if ((params.sortOrder ?? '').toLowerCase() === 'ascending' && by !== 'name') rows.reverse()
  return (
    <>
      <Header icon={<Paperclip size={13} />} title="Attachments" detail={params.patterns ? params.patterns : undefined} />
      {rows.length === 0 ? (
        <Empty q={q} text="No attachments on this page." />
      ) : (
        <ul className="macro-list">
          {rows.map((a) => (
            <li key={a.id}>
              <Paperclip size={14} className="faint" />
              <a href={`/api/attachments/${a.id}/content`} target="_blank" rel="noopener noreferrer">{a.filename}</a>
              <span className="faint small">{formatBytes(a.size)} · {formatDate(a.createdAt)}{a.createdByName ? ` · ${a.createdByName}` : ''}</span>
              <a className="icon-btn" title="Download" href={`/api/attachments/${a.id}/content`} download={a.filename}><Download size={14} /></a>
            </li>
          ))}
        </ul>
      )}
    </>
  )
}

interface TreeNode {
  id: string
  parentId: string | null
  title: string
}

function PageTree({ params, page }: { params: Params; page: Page | undefined }) {
  const q = useQuery({
    queryKey: ['tree', page?.spaceKey],
    queryFn: () => api.get<{ homePageId: string | null; nodes: TreeNode[] }>(`/spaces/${page!.spaceKey}/tree`),
    enabled: !!page,
  })
  const nodes = q.data?.nodes ?? []
  const byParent = new Map<string, TreeNode[]>()
  for (const n of nodes) byParent.set(n.parentId ?? '', [...(byParent.get(n.parentId ?? '') ?? []), n])
  const root = params.root ?? '@home'
  const rootId = root === '@self' ? page?.id : root === '@parent' ? page?.parentId ?? '' : root === '@home' || !root ? q.data?.homePageId ?? '' : nodes.find((n) => n.title.toLowerCase() === root.toLowerCase())?.id ?? ''
  const render = (parent: string, level: number): React.ReactNode => {
    const kids = byParent.get(parent)
    if (!kids?.length || level > 30) return null
    return (
      <ul>
        {kids.map((k) => (
          <li key={k.id}>
            <Link to={`/pages/${k.id}`} className={k.id === page?.id ? 'current' : undefined}>{k.title}</Link>
            {render(k.id, level + 1)}
          </li>
        ))}
      </ul>
    )
  }
  return (
    <>
      <Header icon={<ListTree size={13} />} title="Page tree" />
      {nodes.length === 0 ? <Empty q={q} text="No pages." /> : <div className="macro-tree">{render(rootId ?? '', 0) ?? <div className="faint small">No child pages.</div>}</div>}
    </>
  )
}

/** Live view for a supported macro; `null` for macros without one. */
export function LiveMacro({ name, params, pageId }: { name: string; params: Params; pageId: string }) {
  const page = useQuery({ queryKey: ['page', pageId], queryFn: () => api.get<Page>(`/pages/${pageId}`), enabled: !!pageId, staleTime: 60_000 })
  switch (name) {
    case 'contentbylabel':
      return <PageList params={params} page={page.data} mode="label" />
    case 'recently-updated':
      return <PageList params={params} page={page.data} mode="recent" />
    case 'detailssummary':
    case 'content-report-table':
      return <PropertiesReport params={params} page={page.data} />
    case 'tasks-report-macro':
      return <TasksReport params={params} page={page.data} />
    case 'decisionreport':
      return <DecisionsReport params={params} page={page.data} />
    case 'attachments':
      return <AttachmentsList params={params} pageId={pageId} />
    case 'pagetree':
      return <PageTree params={params} page={page.data} />
    default:
      return (
        <div className="block-label">
          <Puzzle size={13} /> {name}
        </div>
      )
  }
}
