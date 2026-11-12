// Live rendering of Confluence's dynamic macros (imported as `macro` nodes): content by label,
// recently updated, page properties report, task report, attachments and page tree. Other
// macros keep the placeholder view.

import { t as tr } from '../i18n'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate } from 'react-router-dom'
import { CheckSquare, CircleDot, Clock, Download, ExternalLink, FilePlus2, FileText, ListTree, Milestone, Paperclip, Puzzle, Search, SlidersHorizontal, Table2, Tag } from 'lucide-react'
import TemplatePicker from '../components/TemplatePicker'
import { useToast } from '../ui'
import { api, formatBytes, formatDate, formatDateOnly, type Attachment, type Page, type PageProperty } from '../api'
import { PropertyValue } from '../components/PageProperties'

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

export const LIVE_MACROS = new Set(['contentbylabel', 'recently-updated', 'detailssummary', 'content-report-table', 'tasks-report-macro', 'decisionreport', 'attachments', 'pagetree', 'livesearch', 'create-from-template', 'jira', 'jiraissues'])

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
  if (q.error) return <div className="faint small">{tr('Could not load:')}{' '}{q.error instanceof Error ? q.error.message : tr('error')}</div>
  return <div className="faint small">{text}</div>
}

function PageList({ params, page, mode }: { params: Params; page: Page | undefined; mode: 'label' | 'recent' }) {
  const spec = querySpec(params, page?.spaceKey)
  // recently-updated defaults to the current space; contentbylabel to all spaces
  const spaces = spec.spaces.length ? spec.spaces : mode === 'recent' && !(params.spaces ?? '').includes('@all') && page ? [page.spaceKey] : []
  const max = Number(params.max ?? params.maxResults ?? (mode === 'recent' ? 15 : 20)) || 20
  const sort = mode === 'recent' ? 'updated' : params.sort === 'title' || !params.sort ? 'title' : params.sort === 'creation' ? 'created' : 'updated'
  // a CQL query is run as such on the server; its labels and spaces are the fallback
  const cql = (params.cql ?? '').trim()
  const enabled = mode === 'recent' || spec.labels.length > 0 || !!cql
  const q = useQuery({
    queryKey: ['report-pages', mode, spec.labels, spaces, spec.all, sort, max, cql],
    queryFn: () =>
      api.get<ReportPage[]>(
        `/reports/pages?${qs({ labels: spec.labels.join(','), spaces: spaces.join(','), operator: spec.all ? 'all' : undefined, sort, max, cql: cql || undefined, space: page?.spaceKey })}`,
      ),
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
        title={mode === 'recent' ? tr('Recently updated') : tr('Content by label')}
        detail={cql ? cql : [spec.labels.length ? `labels: ${spec.labels.join(spec.all ? ' and ' : ', ')}` : '', spaces.length ? `in ${spaces.join(', ')}` : ''].filter(Boolean).join(' ')}
      />
      {!enabled ? (
        <div className="faint small">{tr('No labels in this macro.')}</div>
      ) : rows.length === 0 ? (
        <Empty q={q} text={tr('No pages found.')} />
      ) : (
        <ul className="macro-list">
          {rows.map((r) => (
            <li key={r.id}>
              <FileText size={14} className="faint" />
              <Link to={`/pages/${r.id}`}>{r.title}</Link>
              {showSpace && <span className="faint small">{r.spaceName}</span>}
              {mode === 'recent' && <span className="faint small">{tr('updated {date}', { date: formatDate(r.updatedAt) })}{r.updatedByName ? ' ' + tr('by {updatedByName}', { updatedByName: r.updatedByName }) + '' : ''}</span>}
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

function PropertiesReport({ params, page, onParams }: { params: Params; page: Page | undefined; onParams?: (p: Params) => void }) {
  const spec = querySpec(params, page?.spaceKey)
  const max = Number(params.pageSize ?? params.max ?? params.maxResults ?? 100) || 100
  const where = (params.where ?? '').trim()
  const enabled = spec.labels.length > 0 || !!where || spec.spaces.length > 0
  const [editing, setEditing] = useState(false)
  const q = useQuery({
    queryKey: ['report-props', spec.labels, spec.spaces, where, max],
    queryFn: () =>
      api.get<{ columns: string[]; rows: { page: ReportPage; properties: Record<string, string>; values?: Record<string, PageProperty> }[] }>(
        `/reports/properties?${qs({ labels: spec.labels.join(','), spaces: spec.spaces.join(','), where: where || undefined, max, sort: 'title' })}`,
      ),
    enabled,
  })
  const headings = csv(params.headings)
  const columns = headings.length ? headings : (q.data?.columns ?? []).slice(0, 8)
  const find = <T,>(m: Record<string, T> | undefined, col: string): T | undefined => (m ? m[col] ?? Object.entries(m).find(([k]) => k.toLowerCase() === col.toLowerCase())?.[1] : undefined)
  const rows = q.data?.rows ?? []
  const detail = [spec.labels.length ? `label: ${spec.labels.join(', ')}` : '', spec.spaces.length ? `in ${spec.spaces.join(', ')}` : '', where ? `where ${where}` : ''].filter(Boolean).join(' · ')
  return (
    <>
      <div className="row" style={{ gap: 6 }}>
        <Header icon={<Table2 size={13} />} title={tr('Page properties report')} detail={detail || undefined} />
        <span className="spacer" />
        {onParams && (
          <button className="icon-btn" title={tr('Filter and columns')} aria-expanded={editing} onClick={() => setEditing(!editing)}>
            <SlidersHorizontal size={14} />
          </button>
        )}
      </div>
      {editing && onParams && <ReportFilter params={params} onSave={(p) => { onParams(p); setEditing(false) }} />}
      {!enabled ? (
        <div className="faint small">{tr('No label, space or filter in this report.')}</div>
      ) : rows.length === 0 ? (
        <Empty q={q} text={spec.labels.length ? tr('No pages with page properties and this label.') : tr('No pages with matching properties.')} />
      ) : (
        <div className="macro-table-wrap">
          <table className="macro-table">
            <thead>
              <tr>
                <th>{params.firstcolumn || tr('Title')}</th>
                {columns.map((c) => (
                  <th key={c}>{c}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.page.id}>
                  <td><Link to={`/pages/${r.page.id}`}>{r.page.title}</Link></td>
                  {columns.map((c) => {
                    const typed = find(r.values, c)
                    return <td key={c}>{typed && typed.value !== null ? <PropertyValue prop={typed} /> : find(r.properties, c) ?? ''}</td>
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}

/** Editors set which pages a properties report shows and which columns. */
function ReportFilter({ params, onSave }: { params: Params; onSave: (p: Params) => void }) {
  const [labels, setLabels] = useState(params.labels ?? params.label ?? '')
  const [where, setWhere] = useState(params.where ?? '')
  const [headings, setHeadings] = useState(params.headings ?? '')
  const [allSpaces, setAllSpaces] = useState(params.spaces === '@all')
  const save = () => {
    const next: Params = { ...params, labels: labels.trim(), where: where.trim(), headings: headings.trim(), spaces: allSpaces ? '@all' : params.spaces && params.spaces !== '@all' ? params.spaces : '@self' }
    delete next.label
    delete next.cql
    onSave(next)
  }
  return (
    <div className="report-filter" onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); save() } }}>
      <label className="field">
        <span>{tr('Pages with these labels')}{' '}<span className="faint">{tr('(optional, comma-separated)')}</span></span>
        <input className="input" value={labels} onChange={(e) => setLabels(e.target.value)} placeholder={tr('project, active')} />
      </label>
      <label className="field">
        <span>{tr('Where')}</span>
        <input className="input" value={where} onChange={(e) => setWhere(e.target.value)} placeholder={tr('Status = Done; Effort >= 3; Owner ~ Ann')} />
        <span className="faint small">{tr('Conditions on properties, separated by semicolons: = equal, != not equal, ~ contains, < > <= >= for numbers and dates (YYYY-MM-DD).')}</span>
      </label>
      <label className="field">
        <span>{tr('Columns')}{' '}<span className="faint">{tr('(optional, comma-separated property names)')}</span></span>
        <input className="input" value={headings} onChange={(e) => setHeadings(e.target.value)} placeholder={tr('Status, Owner, Due')} />
      </label>
      <label className="row small" style={{ gap: 6 }}>
        <input type="checkbox" checked={allSpaces} onChange={(e) => setAllSpaces(e.target.checked)} />{' '}{tr('All spaces (not only this one)')}</label>
      <div className="row" style={{ gap: 6, marginTop: 8 }}>
        <button className="btn btn-primary btn-sm" onClick={save}>{tr('Apply')}</button>
      </div>
    </div>
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

function TasksReport({ params, page, onParams }: { params: Params; page: Page | undefined; onParams?: (p: Params) => void }) {
  const spec = querySpec(params, page?.spaceKey)
  const spaces = spec.spaces.length ? spec.spaces : page ? [page.spaceKey] : []
  const status = /^complete$/i.test(params.status ?? '') ? 'done' : /all/i.test(params.status ?? '') ? 'all' : 'open'
  const max = Number(params.pageSize ?? 20) || 20
  const assignees = params.assignees ?? params.assignee ?? ''
  const dueFrom = params.duedateFrom ?? ''
  const dueTo = params.duedateTo ?? ''
  const [editing, setEditing] = useState(false)
  const q = useQuery({
    queryKey: ['report-tasks', spaces, spec.labels, status, max, assignees, dueFrom, dueTo],
    queryFn: () =>
      api.get<TaskRow[]>(`/reports/tasks?${qs({ spaces: spaces.join(','), labels: spec.labels.join(','), status, max, assignees: assignees || undefined, dueFrom: dueFrom || undefined, dueTo: dueTo || undefined })}`),
  })
  const rows = q.data ?? []
  const detail = [
    status === 'open' ? 'open tasks' : status === 'done' ? 'completed tasks' : 'all tasks',
    /^(@?me|currentuser\(\))$/i.test(assignees) ? 'assigned to me' : assignees ? 'by assignee' : '',
    dueFrom || dueTo ? `due ${dueFrom ? `from ${dueFrom}` : ''}${dueFrom && dueTo ? ' ' : ''}${dueTo ? `until ${dueTo}` : ''}` : '',
    spaces.length ? `in ${spaces.join(', ')}` : '',
    spec.labels.length ? `labels: ${spec.labels.join(', ')}` : '',
  ].filter(Boolean).join(' ')
  return (
    <>
      <div className="row" style={{ gap: 6 }}>
        <Header icon={<CheckSquare size={13} />} title={tr('Task report')} detail={detail} />
        <span className="spacer" />
        {onParams && (
          <button className="icon-btn" title={tr('Filter tasks')} aria-expanded={editing} onClick={() => setEditing(!editing)}>
            <SlidersHorizontal size={14} />
          </button>
        )}
      </div>
      {editing && onParams && <TaskFilter params={params} status={status} onSave={(p) => { onParams(p); setEditing(false) }} />}
      {rows.length === 0 ? (
        <Empty q={q} text={tr('No tasks.')} />
      ) : (
        <div className="macro-table-wrap">
          <table className="macro-table">
            <thead>
              <tr><th style={{ width: 28 }} /><th>{tr('Task')}</th><th>{tr('Assignee')}</th><th>{tr('Due')}</th><th>{tr('Page')}</th></tr>
            </thead>
            <tbody>
              {rows.map((t) => (
                <tr key={`${t.pageId}-${t.idx}`}>
                  <td><input type="checkbox" checked={t.done} readOnly aria-label={t.done ? tr('done') : tr('open')} /></td>
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

/** Editors pick which tasks a task report lists. */
function TaskFilter({ params, status, onSave }: { params: Params; status: string; onSave: (p: Params) => void }) {
  const [st, setSt] = useState(status)
  const [mine, setMine] = useState(/^(@?me|currentuser\(\))$/i.test(params.assignees ?? params.assignee ?? ''))
  const [from, setFrom] = useState(/^\d{4}-\d{2}-\d{2}$/.test(params.duedateFrom ?? '') ? params.duedateFrom! : '')
  const [to, setTo] = useState(/^\d{4}-\d{2}-\d{2}$/.test(params.duedateTo ?? '') ? params.duedateTo! : '')
  const [labels, setLabels] = useState(params.labels ?? params.label ?? '')
  const save = () => {
    const next: Params = { ...params, status: st === 'done' ? 'complete' : st === 'all' ? 'all' : 'incomplete', labels: labels.trim(), duedateFrom: from, duedateTo: to }
    if (mine) next.assignees = 'me'
    else if (/^(@?me|currentuser\(\))$/i.test(next.assignees ?? '')) delete next.assignees
    delete next.assignee
    delete next.label
    onSave(next)
  }
  return (
    <div className="report-filter">
      <label className="field">
        <span>{tr('Tasks')}</span>
        <select className="select" value={st} onChange={(e) => setSt(e.target.value)}>
          <option value="open">{tr('Open')}</option>
          <option value="done">{tr('Completed')}</option>
          <option value="all">{tr('All')}</option>
        </select>
      </label>
      <label className="row small" style={{ gap: 6 }}>
        <input type="checkbox" checked={mine} onChange={(e) => setMine(e.target.checked)} />{' '}{tr('Only tasks assigned to the reader')}</label>
      <div className="row" style={{ gap: 8, flexWrap: 'wrap' }}>
        <label className="field"><span>{tr('Due from')}</span><input className="input" type="date" value={from} onChange={(e) => setFrom(e.target.value)} /></label>
        <label className="field"><span>{tr('Due until')}</span><input className="input" type="date" value={to} onChange={(e) => setTo(e.target.value)} /></label>
      </div>
      <label className="field">
        <span>{tr('Pages with these labels')}{' '}<span className="faint">{tr('(optional)')}</span></span>
        <input className="input" value={labels} onChange={(e) => setLabels(e.target.value)} placeholder={tr('sprint-12, team-a')} />
      </label>
      <div className="row"><button className="btn btn-primary btn-sm" onClick={save}>{tr('Apply')}</button></div>
    </div>
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
      <Header icon={<Milestone size={13} />} title={tr('Decision log')} detail={[spaces.length ? `in ${spaces.join(', ')}` : '', spec.labels.length ? `labels: ${spec.labels.join(', ')}` : ''].filter(Boolean).join(' ')} />
      {rows.length === 0 ? (
        <Empty q={q} text={tr('No decisions recorded yet. Add one with /decision on any page.')} />
      ) : (
        <div className="macro-table-wrap">
          <table className="macro-table">
            <thead>
              <tr><th>{tr('Decision')}</th><th>{tr('Page')}</th><th>{tr('Updated')}</th></tr>
            </thead>
            <tbody>
              {rows.map((d, i) => (
                <tr key={`${d.pageId}-${i}`}>
                  <td>{d.text}{!d.decided && <span className="badge badge-warning" style={{ marginLeft: 6 }}>{tr('open')}</span>}</td>
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
      <Header icon={<Paperclip size={13} />} title={tr('Attachments')} detail={params.patterns ? params.patterns : undefined} />
      {rows.length === 0 ? (
        <Empty q={q} text={tr('No attachments on this page.')} />
      ) : (
        <ul className="macro-list">
          {rows.map((a) => (
            <li key={a.id}>
              <Paperclip size={14} className="faint" />
              <a href={`/api/attachments/${a.id}/content`} target="_blank" rel="noopener noreferrer">{a.filename}</a>
              <span className="faint small">{formatBytes(a.size)} · {formatDate(a.createdAt)}{a.createdByName ? ` · ${a.createdByName}` : ''}</span>
              <a className="icon-btn" title={tr('Download')} href={`/api/attachments/${a.id}/content`} download={a.filename}><Download size={14} /></a>
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
      <Header icon={<ListTree size={13} />} title={tr('Page tree')} />
      {nodes.length === 0 ? <Empty q={q} text={tr('No pages.')} /> : <div className="macro-tree">{render(rootId ?? '', 0) ?? <div className="faint small">{tr('No child pages.')}</div>}</div>}
    </>
  )
}

/** Confluence's livesearch: a search box, limited to a space / label if the macro says so. */
function LiveSearch({ params, page }: { params: Params; page?: Page }) {
  const nav = useNavigate()
  const [q, setQ] = useState('')
  const space = params.spaceKey === '@self' ? page?.spaceKey : params.spaceKey
  const go = (e: React.FormEvent) => {
    e.preventDefault()
    const sp = new URLSearchParams({ q: q.trim() })
    if (space) sp.set('space', space)
    if (params.labels) sp.set('label', params.labels.split(',')[0].trim())
    nav(`/search?${sp}`)
  }
  return (
    <form className="macro-search" onSubmit={go} role="search">
      <Search size={15} />
      <input className="input" placeholder={params.placeholder || (space ? tr('Search {space}', { space }) : tr('Search the wiki'))} aria-label={tr('Search')} value={q} onChange={(e) => setQ(e.target.value)} />
      <button type="submit" className="btn btn-sm">{tr('Search')}</button>
    </form>
  )
}

/** A button that creates a page below this one: from the template it names, or with the
 * Create dialog. */
function CreateFromTemplate({ params, page }: { params: Params; page?: Page }) {
  const nav = useNavigate()
  const qc = useQueryClient()
  const toast = useToast()
  const [picking, setPicking] = useState(false)
  const [busy, setBusy] = useState(false)
  if (!page) return null
  const create = async () => {
    if (!params.templateId) return setPicking(true)
    setBusy(true)
    try {
      const r = await api.post<{ id: string }>('/pages', { spaceKey: page.spaceKey, parentId: page.id, templateId: params.templateId, title: params.title || undefined, draft: true })
      qc.invalidateQueries({ queryKey: ['tree', page.spaceKey] })
      nav(`/pages/${r.id}?new=1`)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not create the page'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="macro-create">
      <button type="button" className="btn btn-primary btn-sm" disabled={busy} onClick={create}>
        <FilePlus2 size={14} /> {params.buttonLabel || tr('Create a page')}
      </button>
      {picking && <TemplatePicker spaceKey={page.spaceKey} parentId={page.id} onClose={() => setPicking(false)} />}
    </div>
  )
}

interface JiraResult {
  columns: string[]
  issues: { key: string; url: string; cells: Record<string, string | null>; statusColor: string }[]
  searchUrl: string | null
  more: boolean
  total?: number | null
  error?: string
}

const JIRA_COLUMN_LABELS: Record<string, string> = {
  key: tr('Key'),
  summary: tr('Summary'),
  type: tr('Type'),
  issuetype: tr('Type'),
  created: tr('Created'),
  updated: tr('Updated'),
  due: tr('Due'),
  duedate: tr('Due'),
  assignee: tr('Assignee'),
  reporter: tr('Reporter'),
  priority: tr('Priority'),
  status: tr('Status'),
  resolution: tr('Resolution'),
}
const JIRA_DATES = new Set(['created', 'updated', 'due', 'duedate', 'resolved', 'resolutiondate'])

/** Confluence's Jira issues macro: the issues a JQL query finds, from the connected Jira. */
function JiraIssues({ params, onParams }: { params: Params; onParams?: (p: Params) => void }) {
  const jql = (params.jqlQuery ?? params.jql ?? '').trim()
  const max = Number(params.maximumIssues ?? 20) || 20
  const q = useQuery({
    queryKey: ['jira-search', jql, params.columns ?? '', max],
    queryFn: () => api.get<JiraResult>(`/unfurl/jira-search?${qs({ jql, columns: params.columns, max })}`),
    enabled: !!jql,
    staleTime: 5 * 60_000,
    retry: false,
  })
  const edit = () => {
    const next = window.prompt(tr('Jira query (JQL)'), jql)
    if (next !== null && next.trim()) onParams?.({ ...params, jqlQuery: next.trim() })
  }
  const d = q.data
  const columns = d?.columns ?? []
  return (
    <>
      <div className="block-label row">
        <CircleDot size={13} /> {tr('Jira issues')}
        {d?.searchUrl && (
          <a className="faint" style={{ textTransform: 'none', fontWeight: 400, marginInlineStart: 'auto' }} href={d.searchUrl} target="_blank" rel="noopener noreferrer">
            {tr('Open in Jira')} <ExternalLink size={11} />
          </a>
        )}
        {onParams && <button type="button" className="btn btn-ghost btn-sm" style={d?.searchUrl ? undefined : { marginInlineStart: 'auto' }} onClick={edit}>{tr('Edit query')}</button>}
      </div>
      {!jql ? (
        <div className="faint small">{tr('No query yet.')}</div>
      ) : q.isLoading ? (
        <div className="spinner" />
      ) : d?.error || q.error ? (
        <div className="faint small">
          <code className="jql">{jql}</code>
          <div>{d?.error === "Jira isn't connected" ? tr('Jira isn’t connected to this wiki, so the query can’t run here. An administrator connects it (WIKICRAFT_JIRA_URL).') : tr('Jira didn’t answer this query.')}</div>
        </div>
      ) : (d?.issues.length ?? 0) === 0 ? (
        <div className="faint small">{tr('No issues match this query.')}</div>
      ) : params.count === 'true' ? (
        <div>{d?.searchUrl ? <a href={d.searchUrl} target="_blank" rel="noopener noreferrer">{tr('{count} issues', { count: d?.total ?? d?.issues.length ?? 0 })}</a> : tr('{count} issues', { count: d?.total ?? d?.issues.length ?? 0 })}</div>
      ) : (
        <>
          <div className="macro-table-wrap jira-table-wrap">
            <table className="macro-table jira-table">
              <thead>
                <tr>{columns.map((c) => <th key={c}>{JIRA_COLUMN_LABELS[c] ?? c}</th>)}</tr>
              </thead>
              <tbody>
                {d!.issues.map((i) => (
                  <tr key={i.key}>
                    {columns.map((c) => {
                      if (c === 'key') return <td key={c} style={{ whiteSpace: 'nowrap' }}><a href={i.url} target="_blank" rel="noopener noreferrer">{i.key}</a></td>
                      const v = i.cells[c]
                      if (c === 'status' && v) return <td key={c}><span className="status-lozenge" data-color={i.statusColor}>{v}</span></td>
                      if (c === 'summary') return <td key={c} className="jira-summary-cell"><a href={i.url} target="_blank" rel="noopener noreferrer" className="jira-summary">{v}</a></td>
                      return <td key={c} className={JIRA_DATES.has(c) ? 'faint' : undefined}>{v ? (JIRA_DATES.has(c) ? formatDateOnly(v) : v) : ''}</td>
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {d!.more && <div className="faint small" style={{ marginTop: 6 }}>{tr('Showing the first {count}.', { count: d!.issues.length })}</div>}
        </>
      )}
    </>
  )
}

/** Live view for a supported macro; `null` for macros without one. */
export function LiveMacro({ name, params, pageId, onParams }: { name: string; params: Params; pageId: string; onParams?: (p: Params) => void }) {
  const page = useQuery({ queryKey: ['page', pageId], queryFn: () => api.get<Page>(`/pages/${pageId}`), enabled: !!pageId, staleTime: 60_000 })
  switch (name) {
    case 'contentbylabel':
      return <PageList params={params} page={page.data} mode="label" />
    case 'recently-updated':
      return <PageList params={params} page={page.data} mode="recent" />
    case 'detailssummary':
    case 'content-report-table':
      return <PropertiesReport params={params} page={page.data} onParams={onParams} />
    case 'tasks-report-macro':
      return <TasksReport params={params} page={page.data} onParams={onParams} />
    case 'decisionreport':
      return <DecisionsReport params={params} page={page.data} />
    case 'attachments':
      return <AttachmentsList params={params} pageId={pageId} />
    case 'pagetree':
      return <PageTree params={params} page={page.data} />
    case 'livesearch':
      return <LiveSearch params={params} page={page.data} />
    case 'create-from-template':
      return <CreateFromTemplate params={params} page={page.data} />
    case 'jira':
    case 'jiraissues':
      return <JiraIssues params={params} onParams={onParams} />
    default:
      return (
        <div className="block-label">
          <Puzzle size={13} /> {name}
        </div>
      )
  }
}
