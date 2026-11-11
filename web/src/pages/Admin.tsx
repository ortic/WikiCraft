import { AVAILABLE, t as tr } from '../i18n'
import { useEffect, useRef, useState } from 'react'
import { Link, NavLink, Navigate, Route, Routes } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Upload, FileSearch, CheckCircle2, XCircle, UserPlus, Users, Activity, Database, RefreshCw, Mail, Send, Globe, Megaphone, Building2, Ban, RotateCcw, Paperclip, Server, Palette } from 'lucide-react'
import { api, formatBytes, formatDate, formatDay, type Job } from '../api'
import { rememberUpload, uploadInParts, type UploadState } from '../upload'
import { Avatar, Modal, Spinner, useMe, useToast, type Features } from '../ui'
import AdminWebhooks from './AdminWebhooks'
import AdminSpaces from './AdminSpaces'
import { TemplateList } from '../components/TemplateManager'

export default function Admin() {
  const me = useMe()
  if (me.data?.user && !['owner', 'admin'].includes(me.data.user.role)) return <div className="empty">{tr('Administrators only.')}</div>
  return (
    <div className="content-pad">
      <h1 style={{ marginTop: 0 }}>{tr('Administration')}</h1>
      <div className="tabs">
        <NavLink end to="/admin" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>{tr('Overview')}</NavLink>
        <NavLink to="/admin/spaces" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>{tr('Spaces')}</NavLink>
        <NavLink to="/admin/users" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>{tr('Users')}</NavLink>
        <NavLink to="/admin/groups" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>{tr('Groups')}</NavLink>
        <NavLink to="/admin/templates" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>{tr('Templates')}</NavLink>
        <NavLink to="/admin/import" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>{tr('Import')}</NavLink>
        <NavLink to="/admin/webhooks" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>{tr('Webhooks')}</NavLink>
        <NavLink to="/admin/audit" className={({ isActive }) => `tab ${isActive ? 'active' : ''}`}>{tr('Audit log')}</NavLink>
      </div>
      <Routes>
        <Route index element={<Overview />} />
        <Route path="spaces" element={<AdminSpaces />} />
        <Route path="users" element={<UsersAdmin />} />
        <Route path="groups" element={<GroupsAdmin />} />
        <Route
          path="templates"
          element={
            <div className="card" style={{ maxWidth: 720 }}>
              <h3 style={{ marginTop: 0 }}>{tr('Templates for every space')}</h3>
              <p className="muted small" style={{ marginTop: 0 }}>{tr('Offered in every space next to the built-in templates and the space’s own.')}</p>
              <TemplateList />
            </div>
          }
        />
        <Route path="import" element={<ImportAdmin />} />
        <Route path="webhooks" element={<AdminWebhooks />} />
        <Route path="audit" element={<Audit />} />
        <Route path="*" element={<Navigate to="/admin" replace />} />
      </Routes>
    </div>
  )
}

function Overview() {
  const toast = useToast()
  const stats = useQuery({ queryKey: ['admin-stats'], queryFn: () => api.get<Record<string, number>>('/admin/stats') })
  const s = stats.data
  const items: [string, string][] = s
    ? [
        [tr('Active users'), String(s.users)],
        [tr('Imported authors (placeholders)'), String(s.placeholderUsers)],
        [tr('Spaces'), String(s.spaces)],
        [tr('Pages'), String(s.pages)],
        [tr('Page versions'), String(s.versions)],
        [tr('Comments'), String(s.comments)],
        [tr('Attachments'), String(s.attachments)],
        [tr('Storage'), formatBytes(s.storageBytes)],
        [tr('Live editing sessions'), String(s.liveRooms)],
      ]
    : []
  return (
    <div>
      {stats.isLoading && <Spinner />}
      <div className="grid-cards">
        {items.map(([k, v]) => (
          <div key={k} className="stat">
            <div className="v">{v}</div>
            <div className="k">{k}</div>
          </div>
        ))}
      </div>
      <div className="row" style={{ marginTop: 24 }}>
        <button
          className="btn"
          onClick={async () => {
            const r = await api.post<{ indexed: number }>('/admin/reindex')
            toast(tr('Re-indexing {indexed} pages', { indexed: r.indexed }))
          }}
        >
          <RefreshCw size={14} />{' '}{tr('Rebuild search index')}</button>
        <Link className="btn" to="/admin/import">
          <Upload size={14} />{' '}{tr('Import from Confluence')}</Link>
      </div>
      <BrandingSettings />
      <LookSettings />
      <WorkspaceSettings />
      <AttachmentSettings />
      <AnnouncementSettings />
      <MailStatus />
      <SystemInfo />
    </div>
  )
}

/** Admin → Overview: workspace name and logo (header, sign-in and shared pages). */
function BrandingSettings() {
  const qc = useQueryClient()
  const toast = useToast()
  const me = useMe()
  const ws = me.data?.workspace
  const [name, setName] = useState<string | null>(null)
  const fileRef = useRef<HTMLInputElement>(null)
  if (!ws) return null
  const value = name ?? ws.name
  const done = (message: string) => {
    qc.invalidateQueries({ queryKey: ['me'] })
    toast(message)
  }
  const fail = (e: unknown) => toast(e instanceof Error ? e.message : tr('Could not save'))
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Building2 size={17} />{' '}{tr('Workspace')}</h3>
      <div className="field">
        <label htmlFor="ws-name">{tr('Name')}</label>
        <div className="row">
          <input id="ws-name" className="input" style={{ maxWidth: 320 }} value={value} maxLength={80} onChange={(e) => setName(e.target.value)} />
          <button
            className="btn"
            disabled={!value.trim() || value === ws.name}
            onClick={async () => {
              try {
                await api.put('/admin/workspace', { name: value })
                setName(null)
                done(tr('Workspace renamed'))
              } catch (e) {
                fail(e)
              }
            }}
          >{tr('Save')}</button>
        </div>
      </div>
      <div className="field" style={{ marginBottom: 0 }}>
        <label>{tr('Logo')}</label>
        <div className="row" style={{ flexWrap: 'wrap' }}>
          <span className="logo-preview">{ws.logoVersion ? <img src={`/api/workspace/logo?v=${encodeURIComponent(ws.logoVersion)}`} alt={tr('Current logo')} /> : <span className="faint small">{tr('WikiCraft mark')}</span>}</span>
          <input
            ref={fileRef}
            type="file"
            accept="image/png,image/jpeg,image/webp"
            hidden
            onChange={async (e) => {
              const f = e.target.files?.[0]
              e.target.value = ''
              if (!f) return
              const form = new FormData()
              form.append('file', f)
              try {
                await api.upload('/admin/workspace/logo', form)
                done(tr('Logo updated'))
              } catch (err) {
                fail(err)
              }
            }}
          />
          <button className="btn" onClick={() => fileRef.current?.click()}>{tr('Upload logo')}</button>
          {ws.logoVersion && (
            <button
              className="btn"
              onClick={async () => {
                try {
                  await api.del('/admin/workspace/logo')
                  done(tr('Logo removed'))
                } catch (e) {
                  fail(e)
                }
              }}
            >{tr('Remove')}</button>
          )}
        </div>
        <p className="faint small" style={{ margin: '6px 0 0' }}>{tr('PNG, JPEG or WebP up to 1 MB, about 26 px high in the header. Shown with the workspace name on every page, the sign-in page and shared pages.')}</p>
      </div>
    </div>
  )
}

const DATE_FORMATS: [string, string][] = [
  ['auto', tr('Reader’s language')],
  ['yyyy-mm-dd', 'YYYY-MM-DD'],
  ['dd.mm.yyyy', 'DD.MM.YYYY'],
  ['dd/mm/yyyy', 'DD/MM/YYYY'],
  ['mm/dd/yyyy', 'MM/DD/YYYY'],
  ['d mmm yyyy', 'D Mon YYYY'],
]

/** Admin → Overview: accent colour, how dates are written and the default time zone. */
function LookSettings() {
  const qc = useQueryClient()
  const toast = useToast()
  const me = useMe()
  const ws = me.data?.workspace
  const zones = (() => {
    try {
      return (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.('timeZone') ?? []
    } catch {
      return []
    }
  })()
  const [color, setColor] = useState<string | null>(null)
  if (!ws) return null
  const save = async (body: Record<string, string>, message: string) => {
    try {
      await api.put('/admin/workspace', body)
      await qc.invalidateQueries({ queryKey: ['me'] })
      toast(message)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  const current = ws.accentColor ?? ''
  const shown = color ?? current
  const sample = new Date(Date.UTC(new Date().getUTCFullYear(), 2, 5, 12))
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Palette size={17} />{' '}{tr('Look and dates')}</h3>
      <div className="field">
        <label htmlFor="ws-accent">{tr('Accent colour')}</label>
        <div className="row" style={{ flexWrap: 'wrap' }}>
          <input type="color" aria-label={tr('Pick a colour')} value={/^#[0-9a-f]{6}$/i.test(shown) ? shown : '#4f46e5'} onChange={(e) => setColor(e.target.value)} style={{ width: 40, height: 32, padding: 2, border: '1px solid var(--border)', borderRadius: 6, background: 'var(--bg)' }} />
          <input id="ws-accent" className="input" style={{ maxWidth: 120, fontFamily: 'var(--mono)' }} placeholder="#4f46e5" value={shown} maxLength={7} onChange={(e) => setColor(e.target.value)} />
          <button className="btn" disabled={shown === current || !/^#[0-9a-f]{6}$/i.test(shown)} onClick={() => save({ accentColor: shown }, tr('Accent colour saved')).then(() => setColor(null))}>{tr('Save')}</button>
          {current && <button className="btn" onClick={() => save({ accentColor: '' }, tr('Back to the built-in colour')).then(() => setColor(null))}>{tr('Use the built-in colour')}</button>}
        </div>
        <p className="faint small" style={{ margin: '6px 0 0' }}>{tr('Buttons, links and highlights. Darkened or lightened where needed so text stays readable in the light and dark themes; the high-contrast theme keeps its own colours.')}</p>
      </div>
      <div className="row" style={{ gap: 16, flexWrap: 'wrap', alignItems: 'flex-start' }}>
        <div className="field" style={{ marginBottom: 0 }}>
          <label htmlFor="ws-datefmt">{tr('Date format')}</label>
          <select id="ws-datefmt" className="input" value={ws.dateFormat ?? 'auto'} onChange={(e) => save({ dateFormat: e.target.value }, tr('Date format saved'))}>
            {DATE_FORMATS.map(([v, label]) => <option key={v} value={v}>{label}</option>)}
          </select>
          <p className="faint small" style={{ margin: '6px 0 0' }}>{tr('For example {date}. Recent changes still read “3 h ago”.', { date: formatDay(sample) })}</p>
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label htmlFor="ws-tz">{tr('Default time zone')}</label>
          <select id="ws-tz" className="input" value={ws.timeZone ?? ''} onChange={(e) => save({ timeZone: e.target.value }, tr('Time zone saved'))}>
            <option value="">{tr('Each reader’s browser')}</option>
            {ws.timeZone && !zones.includes(ws.timeZone) && <option value={ws.timeZone}>{ws.timeZone}</option>}
            {zones.map((z) => <option key={z} value={z}>{z}</option>)}
          </select>
          <p className="faint small" style={{ margin: '6px 0 0' }}>{tr('For people who haven’t set their own in their profile; also when due-task reminders go out.')}</p>
        </div>
        <div className="field" style={{ marginBottom: 0 }}>
          <label htmlFor="ws-lang">{tr('Default language')}</label>
          <select id="ws-lang" className="input" value={ws.defaultLanguage ?? ''} onChange={(e) => save({ defaultLanguage: e.target.value }, tr('Language saved'))}>
            <option value="">{tr('Each reader’s browser')}</option>
            {AVAILABLE.map((l) => <option key={l.code} value={l.code} lang={l.code}>{l.name}</option>)}
          </select>
          <p className="faint small" style={{ margin: '6px 0 0' }}>{tr('The interface language for people who haven’t chosen their own (Settings → Appearance).')}</p>
        </div>
      </div>
    </div>
  )
}

type Announcement = { text: string; level: 'info' | 'warning' | 'success'; id: string }

/** Admin → Overview: the banner everyone sees at the top of the wiki. */
interface SystemState {
  version: string
  node: string
  cluster: boolean
  database: { version: string; sizeBytes: number; schema: number | null; connections: number; idleConnections: number }
  jobs: { queued: number; running: number; failedLastDay: number }
  search: { backend: string; indexed: number | null; pages: number; language: string | null; rebuilding: boolean }
  retrieval?: {
    passages: number
    pages: number
    pagesPending: number
    semantic: { configured: boolean; model?: string; dims?: number | null; embedded?: number; pending?: number; error?: string | null }
  } | null
  mail: { enabled: boolean }
  virusScan?: { enabled: boolean; failOpen: boolean }
  storage: { backend: string }
  liveRooms: number
  signIn: { id: string; kind: string; name: string }[]
}

/** Passages for AI applications and, when configured, how far their embedding has come. */
function retrievalSummary(r: SystemState['retrieval']): React.ReactNode {
  if (!r) return '?'
  const passages = tr('{passages} passages from {pages} pages', { passages: r.passages, pages: r.pages }) + (r.pagesPending ? ' ' + tr('({count} pages waiting)', { count: r.pagesPending }) : '')
  const sem = r.semantic
  if (!sem.configured) return `${passages} · ${tr('keyword only (WIKICRAFT_EMBEDDINGS_URL not set)')}`
  const progress = tr('semantic: {model}, {embedded} embedded, {pending} waiting', { model: sem.model ?? '?', embedded: sem.embedded ?? 0, pending: sem.pending ?? 0 })
  return (
    <>
      {passages} · {progress}
      {sem.error && <div className="error-text small">{sem.error}</div>}
    </>
  )
}

/** Admin → Overview: what this installation runs on and how it's doing. */
function SystemInfo() {
  const sys = useQuery({ queryKey: ['admin-system'], queryFn: () => api.get<SystemState>('/admin/system'), refetchInterval: 30_000 })
  const s = sys.data
  if (!s) return null
  const behind = s.search.indexed !== null && s.search.indexed < s.search.pages
  const rows: [string, React.ReactNode][] = [
    [tr('Version'), `WikiCraft ${s.version}`],
    [tr('Database'), tr('PostgreSQL {version} · {value} · schema {value2} · {value3} of {connections} connections busy', { version: s.database.version, value: formatBytes(s.database.sizeBytes), value2: s.database.schema ?? '?', value3: s.database.connections - s.database.idleConnections, connections: s.database.connections })],
    [tr('Background jobs'), `${s.jobs.queued} queued · ${s.jobs.running} running${s.jobs.failedLastDay ? ' ' + tr('· {failedLastDay} failed in the last day', { failedLastDay: s.jobs.failedLastDay }) + '' : ''}`],
    [
      tr('Search'),
      `${s.search.backend}${s.search.language ? ' · ' + (s.search.language === 'none' ? tr('no word forms') : tr('word forms: {language}', { language: languageName(s.search.language) })) : ''} · ${s.search.indexed ?? '?'} of ${s.search.pages} pages indexed${s.search.rebuilding ? ' ' + tr('(rebuilding)') : behind ? ' ' + tr('(catching up, or rebuild the index)') + '' : ''}`,
    ],
    [tr('Retrieval (AI)'), retrievalSummary(s.retrieval)],
    ['E-mail', s.mail.enabled ? 'configured' : tr('off (WIKICRAFT_SMTP_URL not set)')],
    [tr('File storage'), s.storage.backend],
    [tr('Virus scanning'), s.virusScan?.enabled ? `ClamAV${s.virusScan.failOpen ? ' ' + tr('(files pass when it is down)') + '' : ''}` : tr('off (WIKICRAFT_CLAMAV not set)')],
    [tr('Sign-in'), s.signIn.length ? `passwords and ${s.signIn.map((p) => `${p.name} (${p.kind})`).join(', ')}` : 'passwords'],
    [tr('Replicas'), s.cluster ? tr('clustered (this node {value})', { value: s.node.slice(0, 8) }) : 'single server'],
    [tr('Live editing'), `${s.liveRooms} ${s.liveRooms === 1 ? 'page' : 'pages'} open`],
  ]
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Server size={17} />{' '}{tr('System')}</h3>
      <table className="kv-table">
        <tbody>
          {rows.map(([k, v]) => (
            <tr key={k}><th scope="row">{k}</th><td>{v}</td></tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

interface AttachmentRules {
  maxFileMb: number
  blockedTypes: string[]
  allowedTypes: string[]
  spaceQuotaMb: number
  workspaceQuotaMb: number
}

/** Admin → Overview: largest file, file types and storage limits for attachments. */
function AttachmentSettings() {
  const qc = useQueryClient()
  const toast = useToast()
  const settings = useQuery({
    queryKey: ['admin-settings'],
    queryFn: () => api.get<{ attachments: AttachmentRules; serverMaxUploadMb: number; attachmentStorageBytes: number }>('/admin/settings'),
  })
  const [form, setForm] = useState<{ maxFileMb: string; blockedTypes: string; allowedTypes: string; spaceQuotaMb: string; workspaceQuotaMb: string } | null>(null)
  const s = settings.data
  if (!s?.attachments) return null
  const a = s.attachments
  const f = form ?? {
    maxFileMb: String(a.maxFileMb),
    blockedTypes: a.blockedTypes.join(', '),
    allowedTypes: a.allowedTypes.join(', '),
    spaceQuotaMb: String(a.spaceQuotaMb),
    workspaceQuotaMb: String(a.workspaceQuotaMb),
  }
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setForm({ ...f, [k]: e.target.value })
  const save = async () => {
    const n = (v: string) => Math.max(0, Math.round(Number(v) || 0))
    try {
      await api.put('/admin/settings', {
        attachments: { maxFileMb: Math.min(n(f.maxFileMb), s.serverMaxUploadMb), blockedTypes: f.blockedTypes, allowedTypes: f.allowedTypes, spaceQuotaMb: n(f.spaceQuotaMb), workspaceQuotaMb: n(f.workspaceQuotaMb) },
      })
      qc.invalidateQueries({ queryKey: ['admin-settings'] })
      qc.invalidateQueries({ queryKey: ['features'] })
      setForm(null)
      toast(tr('Attachment rules saved'))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Paperclip size={17} />{' '}{tr('Attachments')}</h3>
      <p className="muted small" style={{ marginTop: 0 }}>{tr('Rules for uploads and for files that come with Markdown, HTML or Notion imports. Attachments take {size} now, all versions counted.', { size: formatBytes(s.attachmentStorageBytes) })}</p>
      <div className="attachment-rules">
        <div className="field">
          <label htmlFor="att-max">{tr('Largest file (MB)')}</label>
          <input id="att-max" className="input" type="number" min={1} max={s.serverMaxUploadMb} value={f.maxFileMb} onChange={set('maxFileMb')} />
          <span className="faint small">{tr('At most {serverMaxUploadMb} MB, the server\'s limit.', { serverMaxUploadMb: s.serverMaxUploadMb })}</span>
        </div>
        <div className="field">
          <label htmlFor="att-space">{tr('Storage per space (MB)')}</label>
          <input id="att-space" className="input" type="number" min={0} value={f.spaceQuotaMb} onChange={set('spaceQuotaMb')} />
          <span className="faint small">{tr('0 = no limit. Admin → Spaces shows what each space uses.')}</span>
        </div>
        <div className="field">
          <label htmlFor="att-ws">{tr('Storage for the whole wiki (MB)')}</label>
          <input id="att-ws" className="input" type="number" min={0} value={f.workspaceQuotaMb} onChange={set('workspaceQuotaMb')} />
          <span className="faint small">{tr('0 = no limit.')}</span>
        </div>
      </div>
      <div className="field">
        <label htmlFor="att-blocked">{tr('Blocked file types')}</label>
        <input id="att-blocked" className="input" placeholder={tr('exe, bat, cmd, msi')} value={f.blockedTypes} onChange={set('blockedTypes')} />
      </div>
      <div className="field">
        <label htmlFor="att-allowed">{tr('Only these file types')}</label>
        <input id="att-allowed" className="input" placeholder={tr('Empty: every type that isn\'t blocked')} value={f.allowedTypes} onChange={set('allowedTypes')} />
      </div>
      <div className="row">
        <span className="faint small">{tr('File types are extensions, separated by commas. Imports by admins (Confluence, backups) aren\'t limited.')}</span>
        <span className="spacer" />
        <button className="btn btn-primary" disabled={!form} onClick={save}>{tr('Save')}</button>
      </div>
    </div>
  )
}

function AnnouncementSettings() {
  const qc = useQueryClient()
  const toast = useToast()
  const current = useQuery({ queryKey: ['announcement'], queryFn: () => api.get<Announcement | null>('/announcement') })
  const [text, setText] = useState<string | null>(null)
  const [level, setLevel] = useState<Announcement['level'] | null>(null)
  if (current.isLoading) return null
  const value = text ?? current.data?.text ?? ''
  const lvl = level ?? current.data?.level ?? 'info'
  const save = async (t: string) => {
    try {
      await api.put('/admin/announcement', { text: t, level: lvl })
      qc.invalidateQueries({ queryKey: ['announcement'] })
      setText(null)
      setLevel(null)
      toast(t.trim() ? tr('Announcement published') : tr('Announcement removed'))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Megaphone size={17} />{' '}{tr('Announcement')}</h3>
      <p className="muted small" style={{ marginTop: 0 }}>{tr('A banner at the top of every page, for maintenance windows, migrations or news. Links are clickable. People can dismiss it until it changes.')}</p>
      <textarea className="input" rows={2} maxLength={500} value={value} placeholder={tr('WikiCraft will be read-only on Saturday 08:00–10:00 for the database upgrade.')} onChange={(e) => setText(e.target.value)} style={{ width: '100%', resize: 'vertical' }} />
      <div className="row" style={{ marginTop: 8 }}>
        <select className="select" style={{ width: 140 }} value={lvl} onChange={(e) => setLevel(e.target.value as Announcement['level'])} aria-label={tr('Style')}>
          <option value="info">{tr('Information')}</option>
          <option value="warning">{tr('Warning')}</option>
          <option value="success">{tr('Good news')}</option>
        </select>
        <span className="spacer" />
        {current.data && <button className="btn" onClick={() => save('')}>{tr('Remove')}</button>}
        <button className="btn btn-primary" onClick={() => save(value)} disabled={!value.trim()}>{tr('Publish')}</button>
      </div>
    </div>
  )
}

function WorkspaceSettings() {
  const qc = useQueryClient()
  const toast = useToast()
  const settings = useQuery({ queryKey: ['admin-settings'], queryFn: () => api.get<Features & { trashRetentionDays: number; viewerList: boolean; requireTwoFactor: 'off' | 'admins' | 'everyone'; drawioUrl: string | null }>('/admin/settings') })
  const [drawio, setDrawio] = useState<string | null>(null)
  const [retention, setRetention] = useState<string | null>(null)
  if (!settings.data) return null
  const set = async (key: keyof Features | 'viewerList', on: boolean, message: string) => {
    try {
      await api.put('/admin/settings', { [key]: on })
      qc.invalidateQueries({ queryKey: ['admin-settings'] })
      qc.invalidateQueries({ queryKey: ['features'] })
      qc.invalidateQueries({ queryKey: ['ai-status'] })
      toast(message)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  const s = settings.data
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Globe size={17} />{' '}{tr('Sharing and features')}</h3>
      <label className="row">
        <input type="checkbox" checked={s.publicLinks} onChange={(e) => set('publicLinks', e.target.checked, e.target.checked ? tr('Public links are allowed') : tr('Public links are turned off — existing links stop working'))} />{tr('Allow public links (anyone with the link can read a shared page)')}</label>
      <p className="faint small" style={{ margin: '2px 0 12px 24px' }}>{tr('Turning this off disables every existing public link immediately; turning it back on restores them.')}</p>
      <label className="row">
        <input type="checkbox" checked={s.exports} onChange={(e) => set('exports', e.target.checked, e.target.checked ? tr('Everyone can export pages and spaces') : tr('Exports are limited to administrators'))} />{tr('Allow exports (Word, Markdown, HTML, PDF and space archives)')}</label>
      <p className="faint small" style={{ margin: '2px 0 12px 24px' }}>{tr('When off, only administrators can export and the export menus are hidden for everyone else. People can still read, copy and print the pages they can see.')}</p>
      <label className="row">
        <input type="checkbox" checked={s.ai} onChange={(e) => set('ai', e.target.checked, e.target.checked ? tr('Ask the wiki is on') : tr('Ask the wiki is off'))} />{tr('Ask the wiki (questions answered by Claude from the pages a person can see)')}</label>
      <p className="faint small" style={{ margin: '2px 0 12px 24px' }}>{tr('Also needs an API key on the server (WIKICRAFT_AI_API_KEY). When off, no page content is sent anywhere.')}</p>
      <label className="row">
        <input type="checkbox" checked={s.personalSpaces} onChange={(e) => set('personalSpaces', e.target.checked, e.target.checked ? tr('People can create personal spaces') : tr('No new personal spaces'))} />{tr('Personal spaces (a private space per person for drafts and notes)')}</label>
      <p className="faint small" style={{ margin: '2px 0 12px 24px' }}>{tr('Turning this off stops new personal spaces; existing ones stay.')}</p>
      <label className="row">
        <input type="checkbox" checked={s.viewerList} onChange={(e) => set('viewerList', e.target.checked, e.target.checked ? tr('Editors can see who viewed a page') : tr('Viewer lists are off'))} />{tr('Show who viewed a page (to the people who can edit it)')}</label>
      <p className="faint small" style={{ margin: '2px 0 12px 24px' }}>{tr('Off by default: it shows people\'s reading habits. View counts are always shown.')}</p>
      <div className="field">
        <label htmlFor="drawio-url">{tr('draw.io diagrams')}</label>
        <div className="row" style={{ flexWrap: 'wrap', gap: 8 }}>
          <input
            id="drawio-url"
            className="input"
            style={{ maxWidth: 360 }}
            placeholder={tr('empty: diagrams off')}
            value={drawio ?? s.drawioUrl ?? ''}
            onChange={(e) => setDrawio(e.target.value)}
          />
          <button
            className="btn btn-sm"
            disabled={drawio === null}
            onClick={async () => {
              try {
                await api.put('/admin/settings', { drawioUrl: drawio ?? '' })
                setDrawio(null)
                qc.invalidateQueries({ queryKey: ['admin-settings'] })
                qc.invalidateQueries({ queryKey: ['features'] })
                toast(drawio ? tr('Diagrams open in this draw.io') : tr('draw.io diagrams are off'))
              } catch (err) {
                toast(err instanceof Error ? err.message : tr('Could not save'))
              }
            }}
          >{tr('Save')}</button>
        </div>
        <p className="faint small" style={{ margin: '4px 0 0' }}>{tr('The draw.io editor in embed mode:')}{' '}<code>https://embed.diagrams.net</code>{' '}{tr('(the default) runs in the browser and doesn\'t store diagrams; a self-hosted draw.io keeps even the editor on your servers. Empty turns diagram editing off (existing diagrams stay visible).')}</p>
      </div>
      <div className="field">
        <label htmlFor="require-2fa">{tr('Two-factor authentication')}</label>
        <select
          id="require-2fa"
          className="select"
          style={{ maxWidth: 360 }}
          value={s.requireTwoFactor}
          onChange={async (e) => {
            const v = e.target.value
            if (v !== 'off' && !confirm(tr('People in this group who haven\'t set up two-factor authentication have to do so before they can continue — you too, if you haven\'t. Continue?'))) return
            try {
              await api.put('/admin/settings', { requireTwoFactor: v })
              qc.invalidateQueries({ queryKey: ['admin-settings'] })
              qc.invalidateQueries({ queryKey: ['me'] })
              toast(v === 'off' ? tr('Two-factor authentication is optional') : v === 'admins' ? tr('Administrators must use two-factor authentication') : tr('Everyone must use two-factor authentication'))
            } catch (err) {
              toast(err instanceof Error ? err.message : tr('Could not save'))
            }
          }}
        >
          <option value="off">{tr('Optional (everyone decides)')}</option>
          <option value="admins">{tr('Required for administrators')}</option>
          <option value="everyone">{tr('Required for everyone')}</option>
        </select>
        <p className="faint small" style={{ margin: '4px 0 0' }}>{tr('Applies to sign-ins with passwords and single sign-on; API tokens are separate.')}</p>
      </div>
      <div className="field" style={{ marginBottom: 0 }}>
        <label htmlFor="trash-retention">{tr('Trash')}</label>
        <div className="row">
          <span className="small">{tr('Delete trashed pages for good after')}</span>
          <input
            id="trash-retention"
            className="input"
            type="number"
            min={0}
            max={3650}
            style={{ width: 90 }}
            value={retention ?? String(s.trashRetentionDays)}
            onChange={(e) => setRetention(e.target.value)}
          />
          <span className="small">{tr('days')}</span>
          <button
            className="btn"
            disabled={retention === null || retention === String(s.trashRetentionDays)}
            onClick={async () => {
              const days = Math.max(0, Math.min(3650, Math.round(Number(retention) || 0)))
              try {
                await api.put('/admin/settings', { trashRetentionDays: days })
                qc.invalidateQueries({ queryKey: ['admin-settings'] })
                setRetention(null)
                toast(days ? tr('Trashed pages are deleted after {days} days', { days }) : tr('Trashed pages are kept until someone deletes them'))
              } catch (e) {
                toast(e instanceof Error ? e.message : tr('Could not save'))
              }
            }}
          >{tr('Save')}</button>
        </div>
        <p className="faint small" style={{ margin: '4px 0 0' }}>{tr('0 keeps them until a space admin deletes them. Deleting removes the pages with their history, comments and attachments.')}</p>
      </div>
    </div>
  )
}

function MailStatus() {
  const toast = useToast()
  const mail = useQuery({ queryKey: ['admin-mail'], queryFn: () => api.get<{ enabled: boolean; from: string; pending: number; failed: number; sentLastDay: number; lastError: string | null }>('/admin/mail') })
  const [busy, setBusy] = useState(false)
  const m = mail.data
  if (!m) return null
  return (
    <div className="card" style={{ marginTop: 24, maxWidth: 720 }}>
      <h3 style={{ marginTop: 0 }} className="row"><Mail size={17} />{' '}{tr('E-mail')}</h3>
      {m.enabled ? (
        <>
          <p className="muted" style={{ marginTop: 0 }}>{tr('Sending as')}{' '}<b>{m.from}</b>{' '}{tr('· {sentLastDay} sent in the last 24 h · {pending} queued', { sentLastDay: m.sentLastDay, pending: m.pending })}{m.failed ? ' ' + tr('· {failed} failed', { failed: m.failed }) + '' : ''}
          </p>
          {m.lastError && <p className="error-text small">{tr('Last error: {lastError}', { lastError: m.lastError })}</p>}
          <button className="btn" disabled={busy} onClick={async () => {
            setBusy(true)
            try {
              const r = await api.post<{ to: string }>('/admin/mail/test', {})
              toast(tr('Test e-mail sent to {to}', { to: r.to }))
            } catch (e) {
              toast(e instanceof Error ? e.message : tr('Sending failed'))
            } finally {
              setBusy(false)
            }
          }}>{busy ? <Spinner /> : <Send size={14} />}{' '}{tr('Send a test e-mail to me')}</button>
        </>
      ) : (
        <p className="muted" style={{ margin: 0 }}>{tr('Not configured. Set')}{' '}<code>WIKICRAFT_SMTP_URL</code>{' '}{tr('(and')}{' '}<code>WIKICRAFT_MAIL_FROM</code>{tr(') to send invitations, password-reset links and notification e-mails. Until then, invitation links are shown to you to share yourself.')}</p>
      )}
    </div>
  )
}

interface UserRow {
  id: string
  name: string
  email: string | null
  role: string
  status: string
  lastLoginAt: string | null
}

/** Admin → Users: who may create an account without an invitation. */
function SignupSettings() {
  const toast = useToast()
  const q = useQuery({ queryKey: ['admin-signup'], queryFn: () => api.get<{ domains: string[]; mailConfigured: boolean }>('/admin/signup') })
  const [value, setValue] = useState<string | null>(null)
  const text = value ?? (q.data?.domains ?? []).join(', ')
  const save = async () => {
    try {
      const r = await api.put<{ domains: string[] }>('/admin/signup', { domains: text.split(/[\s,;]+/).filter(Boolean) })
      setValue(r.domains.join(', '))
      toast(r.domains.length ? tr('Sign-up open for {list}', { list: r.domains.map((d) => '@' + d).join(', ') }) : tr('Sign-up closed: invitation only'))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  if (!q.data) return null
  return (
    <div className="card" style={{ marginBottom: 14 }}>
      <b>{tr('Sign-up')}</b>
      <p className="muted small" style={{ margin: '4px 0 10px' }}>{tr('People with an address at these domains can create an account themselves after confirming their e-mail. Leave empty for invitation only.')}{!q.data.mailConfigured && ' ' + tr('Needs outgoing e-mail (WIKICRAFT_SMTP_URL).') + ''}
      </p>
      <div className="row" style={{ flexWrap: 'wrap' }}>
        <input className="input" style={{ flex: '1 1 260px' }} value={text} placeholder={tr('acme.com, acme.ch')} onChange={(e) => setValue(e.target.value)} disabled={!q.data.mailConfigured} />
        <button className="btn" onClick={save} disabled={!q.data.mailConfigured}>{tr('Save')}</button>
      </div>
    </div>
  )
}

function UsersAdmin() {
  const qc = useQueryClient()
  const toast = useToast()
  const [showPlaceholders, setShowPlaceholders] = useState(false)
  const users = useQuery({ queryKey: ['admin-users', showPlaceholders], queryFn: () => api.get<UserRow[]>(`/users?include_placeholders=${showPlaceholders}`) })
  const [invite, setInvite] = useState(false)
  const [merging, setMerging] = useState<UserRow | null>(null)
  const update = async (id: string, body: Record<string, unknown>) => {
    try {
      await api.patch(`/users/${id}`, body)
      qc.invalidateQueries({ queryKey: ['admin-users'] })
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Failed'))
    }
  }
  return (
    <div>
      <div className="row" style={{ marginBottom: 12 }}>
        <label className="row small">
          <input type="checkbox" checked={showPlaceholders} onChange={(e) => setShowPlaceholders(e.target.checked)} />{' '}{tr('Show imported authors without account')}</label>
        <span className="spacer" />
        <button className="btn btn-primary" onClick={() => setInvite(true)}>
          <UserPlus size={15} />{' '}{tr('Invite / add user')}</button>
      </div>
      <SignupSettings />
      <table className="list">
        <thead>
          <tr><th>{tr('Name')}</th><th>{tr('E-mail')}</th><th>{tr('Role')}</th><th>{tr('Status')}</th><th>{tr('Last login')}</th><th /></tr>
        </thead>
        <tbody>
          {users.data?.map((u) => (
            <tr key={u.id}>
              <td className="row"><Avatar name={u.name} id={u.id} /> {u.name}</td>
              <td className="muted">{u.email ?? '—'}</td>
              <td>
                {u.role === 'owner' ? <span className="badge badge-accent">{tr('owner')}</span> : (
                  <select className="select" style={{ width: 120, height: 28 }} value={u.role} onChange={(e) => update(u.id, { role: e.target.value })}>
                    <option value="admin">{tr('admin')}</option>
                    <option value="member">{tr('member')}</option>
                    <option value="guest">{tr('guest')}</option>
                  </select>
                )}
              </td>
              <td>
                {u.status === 'placeholder' ? <span className="badge">{tr('imported')}</span> : u.status === 'invited' ? (
                  <span className="row" style={{ gap: 6 }}>
                    <span className="badge">{tr('invited')}</span>
                    <button className="btn btn-ghost small" onClick={async () => {
                      try {
                        const r = await api.post<{ emailed: boolean; inviteUrl: string }>('/users/invite', { email: u.email, role: u.role })
                        if (r.emailed) toast(tr('Invitation sent again to {email}', { email: u.email }))
                        else { await navigator.clipboard?.writeText(r.inviteUrl).catch(() => {}); toast(tr('E-mail is not configured — new invitation link copied')) }
                      } catch (e) { toast(e instanceof Error ? e.message : tr('Failed')) }
                    }}>{tr('Resend')}</button>
                  </span>
                ) : u.role === 'owner' ? <span className="badge">{tr('active')}</span> : (
                  <select className="select" style={{ width: 130, height: 28 }} value={u.status} onChange={(e) => update(u.id, { status: e.target.value })}>
                    <option value="active">{tr('active')}</option>
                    <option value="deactivated">{tr('deactivated')}</option>
                  </select>
                )}
              </td>
              <td className="faint small">{formatDate(u.lastLoginAt) || tr('never')}</td>
              <td>
                {u.status === 'placeholder' && (
                  <button className="btn btn-ghost small" title={tr('This imported author is someone who has an account: move their pages, comments and mentions to it')} onClick={() => setMerging(u)}>{tr('Merge into…')}</button>
                )}
                {u.role !== 'owner' && u.status === 'active' && (
                  <button className="btn btn-ghost small" title={tr('Reset two-factor authentication (e.g. lost phone); signs the person out')} onClick={async () => {
                    if (!confirm(tr('Reset two-factor authentication for {name}? They\'ll be signed out and can sign in with their password only.', { name: u.name }))) return
                    try { await api.post(`/users/${u.id}/reset-mfa`); toast(tr('Two-factor authentication reset for {name}', { name: u.name })) } catch (e) { toast(e instanceof Error ? e.message : tr('Failed')) }
                  }}>{tr('Reset 2FA')}</button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {invite && <AddUserDialog onClose={() => setInvite(false)} />}
      {merging && <MergeUserDialog placeholder={merging} onClose={() => setMerging(null)} />}
    </div>
  )
}

/** Imported author (placeholder) → an existing account: authorship, likes, permissions and
 *  mentions move over, later imports map the Confluence account to that person. */
function MergeUserDialog({ placeholder, onClose }: { placeholder: UserRow; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const people = useQuery({ queryKey: ['admin-users', false], queryFn: () => api.get<UserRow[]>('/users?include_placeholders=false') })
  const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, '')
  const options = (people.data ?? []).filter((u) => u.status !== 'placeholder')
  const guess = options.find((u) => norm(u.name) === norm(placeholder.name) || (!!placeholder.email && u.email?.toLowerCase() === placeholder.email.toLowerCase()))
  const [into, setInto] = useState<string>('')
  const target = into || guess?.id || ''
  const [busy, setBusy] = useState(false)
  const merge = async () => {
    const t = options.find((u) => u.id === target)
    if (!t) return
    setBusy(true)
    try {
      await api.post(`/users/${placeholder.id}/merge`, { intoUserId: t.id })
      qc.invalidateQueries({ queryKey: ['admin-users'] })
      toast(tr('{name} merged into {name2}', { name: placeholder.name, name2: t.name }))
      onClose()
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Merge failed'))
      setBusy(false)
    }
  }
  return (
    <Modal
      title={tr('Merge “{name}”', { name: placeholder.name })}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>{tr('Cancel')}</button>
          <button className="btn btn-primary" disabled={!target || busy} onClick={merge}>{busy ? tr('Merging…') : tr('Merge')}</button>
        </>
      }
    >
      <p className="small muted" style={{ marginTop: 0 }}>{tr('Everything this imported author wrote, owned, liked, watched or was granted moves to the account you pick, and mentions of them point to that person. Later imports of the same Confluence account map to them too. The imported author is removed; this can\'t be undone.')}</p>
      <div className="field">
        <label htmlFor="merge-into">{tr('Merge into')}</label>
        <select id="merge-into" className="select" value={target} onChange={(e) => setInto(e.target.value)}>
          <option value="">{tr('Choose a person…')}</option>
          {options.map((u) => (
            <option key={u.id} value={u.id}>{u.name}{u.email ? ` (${u.email})` : ''}</option>
          ))}
        </select>
      </div>
    </Modal>
  )
}

function AddUserDialog({ onClose }: { onClose: () => void }) {
  const qc = useQueryClient()
  const [mode, setMode] = useState<'invite' | 'password'>('invite')
  const [form, setForm] = useState({ name: '', email: '', password: '', role: 'member' })
  const [error, setError] = useState<string | null>(null)
  const [invited, setInvited] = useState<{ emailed: boolean; inviteUrl: string } | null>(null)
  const submit = async () => {
    setError(null)
    try {
      if (mode === 'invite') {
        setInvited(await api.post<{ emailed: boolean; inviteUrl: string }>('/users/invite', { email: form.email, name: form.name || undefined, role: form.role }))
        qc.invalidateQueries({ queryKey: ['admin-users'] })
      } else {
        await api.post('/users', form)
        qc.invalidateQueries({ queryKey: ['admin-users'] })
        onClose()
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : tr('Failed'))
    }
  }
  if (invited)
    return (
      <Modal title={tr('Invitation created')} onClose={onClose} footer={<button className="btn btn-primary" onClick={onClose}>{tr('Done')}</button>}>
        {invited.emailed ? (
          <p style={{ marginTop: 0 }}>{tr('An invitation was e-mailed to')}{' '}<b>{form.email}</b>{tr('. The link is valid for 7 days.')}</p>
        ) : (
          <p style={{ marginTop: 0 }}>{tr('E-mail isn\'t configured on this server, so share this invitation link with')}{' '}<b>{form.email}</b>{' '}{tr('yourself (valid for 7 days):')}</p>
        )}
        <div className="row">
          <input className="input" readOnly value={invited.inviteUrl} onFocus={(e) => e.target.select()} />
          <button className="btn" onClick={() => navigator.clipboard?.writeText(invited.inviteUrl)}>{tr('Copy')}</button>
        </div>
      </Modal>
    )
  return (
    <Modal
      title={tr('Add people')}
      onClose={onClose}
      footer={<><button className="btn" onClick={onClose}>{tr('Cancel')}</button><button className="btn btn-primary" onClick={submit}>{mode === 'invite' ? tr('Send invitation') : tr('Create')}</button></>}
    >
      <div className="tabs" style={{ marginTop: -6 }}>
        <button className={`tab ${mode === 'invite' ? 'active' : ''}`} onClick={() => setMode('invite')}>{tr('Invite by e-mail')}</button>
        <button className={`tab ${mode === 'password' ? 'active' : ''}`} onClick={() => setMode('password')}>{tr('Create with password')}</button>
      </div>
      <p className="muted small" style={{ marginTop: 0 }}>
        {mode === 'invite' ? '' + tr('They get an e-mail with a link to choose their name and password.') + ' ' : ''}{tr('If an imported Confluence author has this e-mail, the account is linked to their pages and comments.')}</p>
      <div className="field"><label>{tr('E-mail')}</label><input className="input" type="email" value={form.email} onChange={(e) => setForm({ ...form, email: e.target.value })} autoFocus /></div>
      <div className="field"><label>{tr('Name')}{mode === 'invite' ? ' ' + tr('(optional)') + '' : ''}</label><input className="input" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} /></div>
      {mode === 'password' && <div className="field"><label>{tr('Initial password')}</label><input className="input" type="password" value={form.password} onChange={(e) => setForm({ ...form, password: e.target.value })} /></div>}
      <div className="field">
        <label>{tr('Role')}</label>
        <select className="select" value={form.role} onChange={(e) => setForm({ ...form, role: e.target.value })}>
          <option value="member">{tr('Member')}</option>
          <option value="admin">{tr('Admin')}</option>
          <option value="guest">{tr('Guest (only explicitly shared spaces)')}</option>
        </select>
      </div>
      {error && <p className="error-text">{error}</p>}
    </Modal>
  )
}

function GroupsAdmin() {
  const qc = useQueryClient()
  const groups = useQuery({ queryKey: ['groups'], queryFn: () => api.get<{ id: string; name: string; memberCount: number; members: { id: string; name: string }[] | null }[]>('/groups') })
  const users = useQuery({ queryKey: ['users-all'], queryFn: () => api.get<{ id: string; name: string }[]>('/users') })
  const [name, setName] = useState('')
  const [adding, setAdding] = useState<Record<string, string>>({})
  const refresh = () => qc.invalidateQueries({ queryKey: ['groups'] })
  return (
    <div>
      <div className="row" style={{ marginBottom: 16 }}>
        <input className="input" style={{ maxWidth: 280 }} placeholder={tr('New group name')} value={name} onChange={(e) => setName(e.target.value)} />
        <button className="btn btn-primary" disabled={!name.trim()} onClick={async () => { await api.post('/groups', { name }); setName(''); refresh() }}>
          <Users size={15} />{' '}{tr('Create group')}</button>
      </div>
      {groups.data?.map((g) => (
        <div key={g.id} className="card" style={{ marginBottom: 10 }}>
          <div className="row">
            <b>{g.name}</b> <span className="badge">{tr('{memberCount} members', { memberCount: g.memberCount })}</span>
            <span className="spacer" />
            <button className="btn btn-sm btn-danger" onClick={async () => { if (confirm(tr('Delete group {name}?', { name: g.name }))) { await api.del(`/groups/${g.id}`); refresh() } }}>{tr('Delete')}</button>
          </div>
          <div className="row" style={{ flexWrap: 'wrap', marginTop: 8 }}>
            {g.members?.map((m) => (
              <span key={m.id} className="label-chip row" style={{ gap: 4 }}>
                {m.name}
                <a style={{ cursor: 'pointer' }} onClick={async () => { await api.del(`/groups/${g.id}/members/${m.id}`); refresh() }}>×</a>
              </span>
            ))}
            <select className="select" style={{ width: 200, height: 28 }} value={adding[g.id] ?? ''} onChange={(e) => setAdding({ ...adding, [g.id]: e.target.value })}>
              <option value="">{tr('Add member…')}</option>
              {users.data?.filter((u) => !g.members?.some((m) => m.id === u.id)).map((u) => <option key={u.id} value={u.id}>{u.name}</option>)}
            </select>
            <button className="btn btn-sm" disabled={!adding[g.id]} onClick={async () => { await api.post(`/groups/${g.id}/members`, { userId: adding[g.id] }); setAdding({ ...adding, [g.id]: '' }); refresh() }}>{tr('Add')}</button>
          </div>
        </div>
      ))}
      {groups.data?.length === 0 && <div className="empty">{tr('No groups yet.')}</div>}
    </div>
  )
}

/** `attachmentBinariesMissing` / `skipped_page_draft` → "attachment binaries missing" / "skipped page draft" */
function humanize(k: string): string {
  return k.replace(/_/g, ' ').replace(/([a-z])([A-Z])/g, '$1 $2').toLowerCase()
}

function ReportView({ result }: { result: any }) {
  const created = result?.created ?? {}
  const links = result?.links ?? result?.conversion ?? {}
  const conv = result?.conversion ?? {}
  const macros: [string, [number, string | null]][] = Object.entries(conv.macros ?? {}) as any
  macros.sort((a, b) => b[1][0] - a[1][0])
  const counts = result?.report?.counts ?? {}
  return (
    <div>
      {result?.spaces && (
        <div className="row" style={{ flexWrap: 'wrap', marginBottom: 12 }}>
          {result.spaces.map((s: any) => (
            <Link key={s.key} className="btn" to={s.homePageId ? `/pages/${s.homePageId}` : `/spaces/${s.key}`}>{tr('Open {value} ({key})', { value: s.name ?? s.key, key: s.key })}</Link>
          ))}
          {result.spaces.filter((s: any) => s.anonymousAccessInConfluence).map((s: any) => (
            <span key={s.key + '-anon'} className="badge" title={tr('Imported spaces are never made public automatically')}>
              {tr('{key} was public in Confluence — enable anonymous access in space settings if wanted', { key: s.key })}</span>
          ))}
        </div>
      )}
      {result?.conflicts?.length > 0 && (
        <div className="card" style={{ marginBottom: 12 }}>
          <b>{tr('Left as they are ({length})', { length: result.conflicts.length })}</b>
          <p className="muted small" style={{ marginTop: 4 }}>{tr('These pages changed in Confluence too. Compare them, then copy over what\'s needed, or run the update again with “take Confluence\'s version”.')}</p>
          <ul className="small" style={{ margin: 0 }}>
            {result.conflicts.map((c: any) => (
              <li key={c.id}><Link to={`/pages/${c.id}`}>{c.title}</Link> <span className="faint">({c.reason})</span></li>
            ))}
          </ul>
        </div>
      )}
      <div className="grid-cards">
        {Object.entries({ ...counts, ...created }).filter(([, v]) => typeof v === 'number').map(([k, v]) => (
          <div key={k} className="stat"><div className="v">{String(v)}</div><div className="k">{humanize(k)}</div></div>
        ))}
      </div>
      <h4>{tr('Links and references')}</h4>
      <table className="list small">
        <tbody>
          {conv.page_links !== undefined && <tr><td>{tr('Page links resolved')}</td><td>{conv.page_links_resolved} / {conv.page_links}</td></tr>}
          {conv.attachment_refs !== undefined && <tr><td>{tr('Attachment references resolved')}</td><td>{conv.attachment_refs_resolved} / {conv.attachment_refs}</td></tr>}
          {conv.user_refs !== undefined && <tr><td>{tr('User mentions resolved')}</td><td>{conv.user_refs_resolved} / {conv.user_refs}</td></tr>}
          {links.inlineAnchors !== undefined && <tr><td>{tr('Inline comments anchored')}</td><td>{links.inlineAnchors}</td></tr>}
          {conv.lenient_parses !== undefined && <tr><td>{tr('Bodies needing lenient parsing')}</td><td>{conv.lenient_parses}</td></tr>}
        </tbody>
      </table>
      {macros.length > 0 && (
        <>
          <h4>{tr('Macros')}</h4>
          <table className="list small">
            <thead><tr><th>{tr('Macro')}</th><th>{tr('Count')}</th><th>{tr('Support')}</th></tr></thead>
            <tbody>
              {macros.map(([name, [n, support]]) => (
                <tr key={name}>
                  <td><code>{name}</code></td>
                  <td>{n}</td>
                  <td><span className={`badge ${support === 'native' ? 'badge-accent' : ''}`}>{support ?? '?'}</span></td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}
      {result?.report?.warnings?.length > 0 && (
        <>
          <h4>{tr('Warnings')}</h4>
          <ul className="small">{result.report.warnings.slice(0, 50).map((w: string, i: number) => <li key={i}>{w}</li>)}</ul>
        </>
      )}
      {conv.missing_link_samples?.length > 0 && (
        <details>
          <summary className="small muted">{tr('Links to pages not contained in the export ({page_links_missing})', { page_links_missing: conv.page_links_missing })}</summary>
          <ul className="small">{conv.missing_link_samples.map((s: string, i: number) => <li key={i}>{s}</li>)}</ul>
        </details>
      )}
    </div>
  )
}

function ImportAdmin() {
  const toast = useToast()
  const qc = useQueryClient()
  const file = useRef<HTMLInputElement>(null)
  const [selected, setSelected] = useState<File | null>(null)
  const [analysis, setAnalysis] = useState<any>(null)
  const [busy, setBusy] = useState<'analyze' | 'import' | null>(null)
  const [spaceKey, setSpaceKey] = useState('')
  const [skipHistory, setSkipHistory] = useState(false)
  const [skipDrafts, setSkipDrafts] = useState(false)
  const [skipTrash, setSkipTrash] = useState(false)
  // bring an earlier import of the same spaces up to date instead of creating new spaces
  const [update, setUpdate] = useState(false)
  const [overwrite, setOverwrite] = useState(false)
  const [jobId, setJobId] = useState<string | null>(null)
  // the export goes up in parts as soon as it's chosen; the dry run and the import use that upload
  const [upload, setUpload] = useState<UploadState | null>(null)
  const [sent, setSent] = useState<number | null>(null)
  const [uploadError, setUploadError] = useState<string | null>(null)
  const uploading = useRef<AbortController | null>(null)
  useEffect(() => () => uploading.current?.abort(), [])
  const send = async (f: File) => {
    uploading.current?.abort()
    const ctl = new AbortController()
    uploading.current = ctl
    setUpload(null)
    setUploadError(null)
    setSent(0)
    try {
      const u = await uploadInParts(f, (received) => setSent(received), ctl.signal)
      setUpload(u)
    } catch (e) {
      if (!ctl.signal.aborted) setUploadError(e instanceof Error ? e.message : tr('Upload failed'))
    } finally {
      if (uploading.current === ctl) uploading.current = null
      setSent(null)
    }
  }
  const jobs = useQuery({ queryKey: ['jobs'], queryFn: () => api.get<Job[]>('/jobs'), refetchInterval: 5000 })
  const job = useQuery({ queryKey: ['job', jobId], queryFn: () => api.get<Job>(`/jobs/${jobId}`), enabled: !!jobId, refetchInterval: (q) => (q.state.data && ['succeeded', 'failed', 'cancelled'].includes(q.state.data.status) ? false : 1000) })
  useEffect(() => {
    if (job.data?.status === 'succeeded') {
      qc.invalidateQueries({ queryKey: ['spaces'] })
    }
  }, [job.data?.status, qc])

  const analyze = async () => {
    if (!upload) return
    setBusy('analyze')
    setAnalysis(null)
    try {
      const a = await api.post<any>(`/uploads/${upload.id}/analyze-confluence`)
      setAnalysis(a)
      setUpdate(!!a?.spaces?.some((s: any) => s.earlierImport))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Analysis failed'))
    } finally {
      setBusy(null)
    }
  }
  const start = async () => {
    if (!selected || !upload) return
    setBusy('import')
    try {
      const options = { spaceKey: update ? undefined : spaceKey || undefined, skipHistory, skipDrafts, skipTrash, update, overwriteLocalChanges: update && overwrite }
      const r = await api.post<{ jobId: string }>(`/uploads/${upload.id}/import-confluence`, { options })
      // the upload is used up
      rememberUpload(selected, null)
      setUpload(null)
      setSelected(null)
      setJobId(r.jobId)
      qc.invalidateQueries({ queryKey: ['jobs'] })
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Import failed'))
    } finally {
      setBusy(null)
    }
  }
  const j = job.data
  return (
    <div>
      <div className="card" style={{ marginBottom: 18 }}>
        <h3 style={{ marginTop: 0 }}>{tr('Import from Confluence')}</h3>
        <p className="muted" style={{ marginTop: 0 }}>{tr('Upload a Confluence')}{' '}<b>{tr('space export')}</b>{' '}{tr('(.zip). Both the classic XML format (Server/Data Center, older Cloud) and the new Cloud CSV format are supported. Pages, hierarchy, history, attachments, comments (including inline comments), labels, restrictions and authors are imported; links and mentions are rewritten. Run an analysis first to see what will be imported.')}</p>
        <input
          ref={file}
          type="file"
          accept=".zip"
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0] ?? null
            setSelected(f)
            setAnalysis(null)
            if (f) send(f)
            e.target.value = ''
          }}
        />
        <div className="row" style={{ flexWrap: 'wrap' }}>
          <button className="btn" onClick={() => file.current?.click()}><Upload size={15} /> {selected ? selected.name : tr('Choose export (.zip)')}</button>
          {selected && <span className="faint small">{formatBytes(selected.size)}</span>}
          <span className="spacer" />
          <button className="btn" disabled={!upload || !!busy} onClick={analyze}>{busy === 'analyze' ? <Spinner /> : <FileSearch size={15} />}{' '}{tr('Analyze (dry run)')}</button>
        </div>
        {selected && sent !== null && (
          <div style={{ marginTop: 12 }}>
            <div className="row small">
              <span>{tr('Uploading: {sent} of {total}', { sent: formatBytes(sent), total: formatBytes(selected.size) })}</span>
              <span className="spacer" />
              <button className="btn btn-ghost btn-sm" onClick={() => uploading.current?.abort()}>{tr('Stop')}</button>
            </div>
            <div className="progress" style={{ marginTop: 6 }} role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round((sent / Math.max(1, selected.size)) * 100)}>
              <div style={{ width: `${Math.round((sent / Math.max(1, selected.size)) * 100)}%` }} />
            </div>
          </div>
        )}
        {selected && sent === null && !upload && (
          <div className="row small" style={{ marginTop: 12 }}>
            {uploadError ? <span className="error-text">{uploadError}</span> : <span className="faint">{tr('The upload stopped.')}</span>}
            <button className="btn btn-sm" onClick={() => send(selected)}>{tr('Continue the upload')}</button>
          </div>
        )}
        {selected && (
          <div className="row" style={{ marginTop: 14, flexWrap: 'wrap' }}>
            <input className="input" style={{ width: 200 }} placeholder={tr('Space key (optional)')} value={spaceKey} onChange={(e) => setSpaceKey(e.target.value.toUpperCase())} />
            <label className="row small"><input type="checkbox" checked={skipHistory} onChange={(e) => setSkipHistory(e.target.checked)} />{' '}{tr('Skip page history')}</label>
            <label className="row small" title={tr('Unpublished pages otherwise become private drafts that only their author sees')}><input type="checkbox" checked={skipDrafts} onChange={(e) => setSkipDrafts(e.target.checked)} />{' '}{tr('Skip unpublished drafts')}</label>
            <label className="row small" title={tr('Pages in the Confluence trash otherwise go to the space trash, where admins can restore them')}><input type="checkbox" checked={skipTrash} onChange={(e) => setSkipTrash(e.target.checked)} />{' '}{tr('Skip trash')}</label>
            <span className="spacer" />
            <button className="btn btn-primary" disabled={!!busy || !upload} onClick={start}>{busy === 'import' ? <Spinner /> : <Upload size={15} />} {update ? tr('Update import') : tr('Start import')}</button>
          </div>
        )}
        {selected && (
          <div className="update-import">
            <label className="row small">
              <input type="checkbox" checked={update} onChange={(e) => setUpdate(e.target.checked)} />{tr('Update an earlier import of these spaces with what changed in Confluence since')}{analysis?.spaces?.filter((s: any) => s.earlierImport).map((s: any) => (
                <span key={s.key} className="badge">{s.key} → {s.earlierImport.key}</span>
              ))}
            </label>
            {update && (
              <label className="row small">
                <input type="checkbox" checked={overwrite} onChange={(e) => setOverwrite(e.target.checked)} />{tr('Pages changed both here and in Confluence take Confluence\'s version (otherwise they\'re left as they are and listed)')}</label>
            )}
          </div>
        )}
      </div>

      <MarkdownImport />

      <RestoreBackup onJob={(id) => { setJobId(id); qc.invalidateQueries({ queryKey: ['jobs'] }) }} />

      {j && (
        <div className="card" style={{ marginBottom: 18 }}>
          <div className="row">
            {j.status === 'succeeded' ? <CheckCircle2 color="var(--success)" size={18} /> : j.status === 'failed' ? <XCircle color="var(--danger)" size={18} /> : j.status === 'cancelled' ? <Ban size={18} className="faint" /> : <Spinner />}
            <b>{j.kind === 'export.space' ? tr('Export') : j.kind === 'import.backup' ? tr('Restore') : tr('Import')} {j.cancelling ? tr('stopping') : j.status}</b>
            <span className="faint small">{j.progress?.phase}{j.progress?.total ? ` · ${j.progress.done ?? 0}/${j.progress.total}` : ''}{(j.attempts ?? 1) > 1 ? ' ' + tr('· attempt {attempts}', { attempts: j.attempts }) + '' : ''}</span>
            <span className="spacer" />
            <JobActions job={j} />
          </div>
          {j.progress?.total ? <div className="progress" style={{ marginTop: 10 }}><div style={{ width: `${Math.round(((j.progress.done ?? 0) / j.progress.total) * 100)}%` }} /></div> : null}
          {j.error && j.status !== 'cancelled' && <p className="error-text">{j.error}</p>}
          {j.retryable && j.kind === 'import.confluence' && (
            <p className="faint small" style={{ marginBottom: 0 }}>{tr('Retrying continues this import: what it already brought in is updated, not duplicated.')}</p>
          )}
          {j.status === 'succeeded' && j.kind === 'import.backup' && (
            <p style={{ marginBottom: 0 }}>{tr('Restored')}{' '}<Link to={`/spaces/${j.result?.spaceKey}`}>{j.result?.spaceKey}</Link>{tr(': {pages} pages, {attachments} attachments, {comments} comments', { pages: j.result?.pages, attachments: j.result?.attachments, comments: j.result?.comments })}{j.result?.unmappedRestrictions ? ' ' + tr('· {unmappedRestrictions} restrictions referred to unknown people or groups and now allow only you', { unmappedRestrictions: j.result.unmappedRestrictions }) + '' : ''}
            </p>
          )}
          {j.status === 'succeeded' && j.kind === 'export.space' && (
            <p style={{ marginBottom: 0 }}>{j.result?.file ? <a href={`/api/jobs/${j.id}/download`}>{j.result.filename}</a> : tr('This export has expired.')}</p>
          )}
          {j.status === 'succeeded' && j.kind === 'import.confluence' && <div style={{ marginTop: 14 }}><ReportView result={j.result} /></div>}
        </div>
      )}

      {analysis && (
        <div className="card" style={{ marginBottom: 18 }}>
          <h3 style={{ marginTop: 0 }}>{tr('Analysis')}</h3>
          <p className="muted small">{tr('Format:')}{' '}<b>{analysis.format}</b>{' '}{tr('· source:')}{' '}{analysis.descriptor?.source ?? tr('server')}{' '}{tr('· spaces:')}{' '}{analysis.spaces?.map((s: any) => tr('{name} ({key}, {pages} pages)', { name: s.name, key: s.key, pages: s.pages })).join(', ')}{' '}{tr('· {users} users', { users: analysis.users })}</p>
          <ReportView result={analysis} />
        </div>
      )}

      <h3>{tr('Recent jobs')}</h3>
      <table className="list">
        <thead>
          <tr><th>{tr('Job')}</th><th>{tr('Status')}</th><th>{tr('Started')}</th><th><span className="sr-only">{tr('Actions')}</span></th></tr>
        </thead>
        <tbody>
          {jobs.data?.map((jj) => (
            <tr key={jj.id} style={{ cursor: 'pointer' }} onClick={() => setJobId(jj.id)}>
              <td><Database size={14} /> {jj.kind}</td>
              <td>
                <span className={`badge ${jj.status === 'succeeded' ? 'badge-accent' : ''}`}>{jj.cancelling ? tr('stopping') : jj.status}</span>
                {(jj.attempts ?? 1) > 1 && <span className="faint small">{' '}{tr('· attempt {attempts}', { attempts: jj.attempts })}</span>}
              </td>
              <td className="faint small">{formatDate(jj.createdAt)}</td>
              <td style={{ textAlign: 'right' }} onClick={(e) => e.stopPropagation()}>
                <JobActions job={jj} onRetry={() => setJobId(jj.id)} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/** Cancel a queued or running job; retry a failed or cancelled one. */
function JobActions({ job, onRetry }: { job: Job; onRetry?: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const [busy, setBusy] = useState(false)
  const act = async (what: 'cancel' | 'retry') => {
    setBusy(true)
    try {
      const r = await api.post<Job>(`/jobs/${job.id}/${what}`)
      qc.setQueryData(['job', job.id], r)
      qc.invalidateQueries({ queryKey: ['jobs'] })
      if (what === 'retry') onRetry?.()
      toast(what === 'cancel' ? (r.status === 'cancelled' ? tr('Job cancelled') : tr('The job stops at its next step')) : tr('Job queued again'))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('That did not work'))
    } finally {
      setBusy(false)
    }
  }
  if ((job.status === 'queued' || job.status === 'running') && !job.cancelling) {
    return (
      <button className="btn btn-sm" disabled={busy} onClick={() => act('cancel')}>
        <Ban size={13} />{' '}{tr('Cancel')}</button>
    )
  }
  if (job.retryable) {
    return (
      <button className="btn btn-sm" disabled={busy} onClick={() => act('retry')}>
        <RotateCcw size={13} />{' '}{tr('Retry')}</button>
    )
  }
  return null
}

function Audit() {
  const audit = useQuery({ queryKey: ['audit'], queryFn: () => api.get<{ id: number; actorName: string | null; action: string; targetType: string | null; data: any; createdAt: string }[]>('/admin/audit') })
  return (
    <table className="list">
      <thead><tr><th>{tr('When')}</th><th>{tr('Who')}</th><th>{tr('Action')}</th><th>{tr('Details')}</th></tr></thead>
      <tbody>
        {audit.data?.map((a) => (
          <tr key={a.id}>
            <td className="faint small" title={a.createdAt}>{formatDate(a.createdAt)}</td>
            <td>{a.actorName ?? '—'}</td>
            <td><Activity size={13} /> <code>{a.action}</code></td>
            <td className="faint small" style={{ maxWidth: 420, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{JSON.stringify(a.data)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/** Admin → Import: restore a space from a WikiCraft backup archive. */
function RestoreBackup({ onJob }: { onJob: (id: string) => void }) {
  const toast = useToast()
  const file = useRef<HTMLInputElement>(null)
  const [selected, setSelected] = useState<File | null>(null)
  const [spaceKey, setSpaceKey] = useState('')
  const [busy, setBusy] = useState(false)
  const start = async () => {
    if (!selected) return
    setBusy(true)
    try {
      const form = new FormData()
      form.append('file', selected)
      if (spaceKey.trim()) form.append('spaceKey', spaceKey.trim())
      const r = await api.upload<{ jobId: string; spaceKey: string }>('/imports/backup', form)
      toast(tr('Restoring into space {spaceKey}…', { spaceKey: r.spaceKey }))
      onJob(r.jobId)
      setSelected(null)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Restore failed'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="card" style={{ marginBottom: 18 }}>
      <h3 style={{ marginTop: 0 }}>{tr('Restore a space backup')}</h3>
      <p className="muted" style={{ marginTop: 0 }}>{tr('Upload a')}{' '}<b>{tr('full backup')}</b>{' '}{tr('made under Space settings → Export. The space is restored as a new space with its pages, tree, labels, comments, attachments and restrictions; people and groups are matched by e-mail and name.')}</p>
      <input ref={file} type="file" accept=".zip" hidden onChange={(e) => setSelected(e.target.files?.[0] ?? null)} />
      <div className="row" style={{ flexWrap: 'wrap' }}>
        <button className="btn" onClick={() => file.current?.click()}><Upload size={15} /> {selected ? selected.name : tr('Choose backup (.zip)')}</button>
        {selected && <span className="faint small">{formatBytes(selected.size)}</span>}
        <span className="spacer" />
        <input className="input" style={{ width: 220 }} placeholder={tr('New space key (optional)')} value={spaceKey} onChange={(e) => setSpaceKey(e.target.value.toUpperCase())} />
        <button className="btn btn-primary" disabled={!selected || busy} onClick={start}>{busy ? <Spinner /> : <RefreshCw size={15} />}{' '}{tr('Restore')}</button>
      </div>
    </div>
  )
}

/** Admin → Import: Word documents, Markdown/HTML folders, Obsidian vaults, Notion and Confluence HTML exports into a space. */
function MarkdownImport() {
  const toast = useToast()
  const file = useRef<HTMLInputElement>(null)
  const spaces = useQuery({ queryKey: ['spaces'], queryFn: () => api.get<{ key: string; name: string }[]>('/spaces') })
  const [selected, setSelected] = useState<File | null>(null)
  const [space, setSpace] = useState('')
  const [busy, setBusy] = useState(false)
  const [result, setResult] = useState<{ pages: number; attachments: number; skippedFiles?: string[]; space: string } | null>(null)
  const [split, setSplit] = useState('')
  const start = async () => {
    if (!selected || !space) return
    setBusy(true)
    setResult(null)
    try {
      const form = new FormData()
      form.append('spaceKey', space)
      if (split) form.append('splitHeadings', split)
      form.append('file', selected)
      const r = await api.upload<{ pages: number; attachments: number; skippedFiles?: string[] }>('/imports/markdown', form)
      setResult({ ...r, space })
      setSelected(null)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Import failed'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="card" style={{ marginBottom: 18 }}>
      <h3 style={{ marginTop: 0 }}>{tr('Import Word, Markdown, HTML, Notion or Obsidian')}</h3>
      <p className="muted" style={{ marginTop: 0 }}>{tr('Upload a Word document (')}<b>{tr('.docx')}</b>{tr('), a')}{' '}<b>{tr('.md')}</b>{' '}{tr('or')}{' '}<b>{tr('.html')}</b>{' '}{tr('file or a')}{' '}<b>{tr('.zip')}</b>{' '}{tr('of them: a Notion export (“Markdown & CSV” or HTML), an Obsidian vault, a Confluence HTML space export, a folder of Word files or any docs or HTML site folder. Folders become page trees, images and linked files become attachments, links and')}{' '}<code>[[wiki links]]</code>{' '}{tr('are rewritten, Notion databases become tables, and front matter titles and tags are kept. Word headings, lists, tables, images, links, code and footnotes are converted.')}</p>
      <input ref={file} type="file" accept=".zip,.md,.markdown,.docx,.html,.htm" hidden onChange={(e) => setSelected(e.target.files?.[0] ?? null)} />
      <div className="row" style={{ flexWrap: 'wrap' }}>
        <button className="btn" onClick={() => file.current?.click()}><Upload size={15} /> {selected ? selected.name : tr('Choose file (.docx, .md, .html or .zip)')}</button>
        {selected && <span className="faint small">{formatBytes(selected.size)}</span>}
        <span className="spacer" />
        <select className="select" style={{ width: 220 }} value={space} onChange={(e) => setSpace(e.target.value)}>
          <option value="">{tr('Into space…')}</option>
          {spaces.data?.map((s) => <option key={s.key} value={s.key}>{s.name}</option>)}
        </select>
        <button className="btn btn-primary" disabled={!selected || !space || busy} onClick={start}>{busy ? <Spinner /> : <Upload size={15} />}{' '}{tr('Import')}</button>
      </div>
      {selected && /\.(docx|zip)$/i.test(selected.name) && (
        <label className="row small" style={{ gap: 8, marginTop: 8 }}>{tr('Word documents:')}<select className="select" style={{ width: 'auto' }} value={split} onChange={(e) => setSplit(e.target.value)}>
            <option value="">{tr('one page each')}</option>
            <option value="1">{tr('a page per heading 1')}</option>
            <option value="2">{tr('a page per heading 1 and 2')}</option>
          </select>
        </label>
      )}
      {result && (
        <p style={{ marginBottom: 0 }}>
          <CheckCircle2 size={15} color="var(--success)" style={{ verticalAlign: -2 }} />{' '}{tr('Imported {pages} pages and {attachments} attachments into', { pages: result.pages, attachments: result.attachments })}{' '}
          <Link to={`/spaces/${result.space}`}>{result.space}</Link>.
        </p>
      )}
      {!!result?.skippedFiles?.length && (
        <div className="small" style={{ marginTop: 6 }}>
          <span className="muted">{tr('Left out by the attachment rules:')}</span>
          <ul style={{ margin: '4px 0 0' }}>{result.skippedFiles.map((f) => <li key={f}>{f}</li>)}</ul>
        </div>
      )}
    </div>
  )
}

/** `de` → `German` (in the reader's language). */
function languageName(code: string): string {
  try {
    return new Intl.DisplayNames([document.documentElement.lang || 'en'], { type: 'language' }).of(code) ?? code
  } catch {
    return code
  }
}
