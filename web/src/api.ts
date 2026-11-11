// Typed client for the WikiCraft REST API.
import { locale as uiLocale, t } from './i18n'

export class ApiError extends Error {
  status: number
  code: string
  constructor(status: number, code: string, message: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { 'x-wikicraft': '1' }
  let payload: BodyInit | undefined
  if (body instanceof FormData) {
    payload = body
  } else if (body !== undefined) {
    headers['content-type'] = 'application/json'
    payload = JSON.stringify(body)
  }
  const res = await fetch(`/api${path}`, { method, headers, body: payload, credentials: 'same-origin' })
  if (res.status === 204) return undefined as T
  const text = await res.text()
  const data = text ? JSON.parse(text) : undefined
  if (!res.ok) {
    throw new ApiError(res.status, data?.error ?? 'error', data?.message ?? res.statusText)
  }
  return data as T
}

/** Largest attachment the wiki takes (from /features), to refuse big files before uploading them. */
let maxFileMb: number | null = null
export function setMaxFileMb(mb: number | null | undefined) {
  maxFileMb = mb && mb > 0 ? mb : null
}

function checkFiles(form: FormData, path: string) {
  if (!maxFileMb || !/\/attachments$/.test(path)) return
  for (const v of form.values()) {
    if (v instanceof File && v.size > maxFileMb * 1024 * 1024) {
      throw new ApiError(400, 'bad_request', t('{name} is larger than {max} MB, the largest file this wiki takes', { name: v.name, max: maxFileMb }))
    }
  }
}

export const api = {
  get: <T>(p: string) => request<T>('GET', p),
  post: <T>(p: string, b?: unknown) => request<T>('POST', p, b ?? {}),
  put: <T>(p: string, b?: unknown) => request<T>('PUT', p, b ?? {}),
  patch: <T>(p: string, b?: unknown) => request<T>('PATCH', p, b ?? {}),
  del: <T>(p: string, b?: unknown) => request<T>('DELETE', p, b),
  upload: async <T>(p: string, form: FormData) => {
    checkFiles(form, p)
    return request<T>('POST', p, form)
  },
}

// ---- types (mirror the server's camelCase JSON) ----

export type Role = 'none' | 'viewer' | 'commenter' | 'editor' | 'admin'

export interface User {
  id: string
  workspaceId: string
  email: string | null
  name: string
  role: 'owner' | 'admin' | 'member' | 'guest'
  status: string
  avatarUrl: string | null
}

export interface Me {
  user: User | null
  workspace: {
    id: string
    name: string
    logoVersion?: string | null
    /** `#rrggbb` (Admin → Overview → Look), null = the built-in colour */
    accentColor?: string | null
    /** `auto` or a pattern such as `dd.mm.yyyy` */
    dateFormat?: string
    /** IANA name for people without their own time zone */
    timeZone?: string | null
    /** interface language for people without their own; null = the browser's */
    defaultLanguage?: string | null
  } | null
  /** the signed-in person's own time zone (profile) */
  timeZone?: string | null
  /** the signed-in person's interface language (profile); null = the workspace default */
  language?: string | null
  /** the workspace requires two-factor authentication and it isn't set up yet */
  mustSetUpTwoFactor?: boolean
}

export interface SpaceLook {
  /** `#rrggbb`; unset = the workspace's */
  accentColor?: string
  /** the sidebar leaves out the blog */
  hideBlog?: boolean
}

export interface Space {
  id: string
  key: string
  name: string
  description: string | null
  icon: string | null
  category?: string | null
  homePageId: string | null
  defaultRole: Role
  anonymousAccess: boolean
  role: Role
  /** the caller's space permissions */
  perms?: Perm[]
  pageCount: number
  updatedAt: string
  archivedAt?: string | null
  /** the signed-in person starred it */
  starred?: boolean
  /** the person responsible for the space */
  ownerId?: string | null
  ownerName?: string | null
  personalOwnerId?: string | null
  /** links pinned to the space sidebar */
  shortcuts?: Shortcut[]
  /** the space's own look (Space settings → Look) */
  look?: SpaceLook
}

export interface Shortcut {
  title: string
  /** a web address or a path in this wiki */
  url: string
}

export interface TreeNode {
  id: string
  parentId: string | null
  title: string
  kind: string
  icon: string | null
  position: number
  restricted: boolean
  /** kind `link`: the external page this tree item opens */
  url?: string
  /** your unpublished draft */
  draft?: boolean
  /** when the draft publishes itself */
  publishAt?: string | null
}

export interface PageAccess {
  view: boolean
  comment: boolean
  edit: boolean
  admin: boolean
  /** the archived page this one is (or is under) */
  archivedPage?: string | null
  /** the caller's space permissions on this page */
  perms?: Perm[]
  /** the caller created the page ("delete own" covers it) */
  own?: boolean
}

/** Space permissions (Confluence's set): a role brings them, a space grant may leave some out. */
export type Perm =
  | 'page.add'
  | 'page.delete'
  | 'blog.add'
  | 'blog.delete'
  | 'comment.add'
  | 'comment.delete'
  | 'attachment.add'
  | 'attachment.delete'
  | 'restrict'
  | 'export'
  | 'archive'
  | 'delete.own'

/** Whether a space or page access carries a permission (the server decides when it's not sent). */
export function can(x: { perms?: Perm[] } | null | undefined, p: Perm): boolean {
  return !x?.perms || x.perms.includes(p)
}

export interface PmNode {
  type: string
  attrs?: Record<string, unknown>
  content?: PmNode[]
  text?: string
  marks?: { type: string; attrs?: Record<string, unknown> }[]
}

/** A page that links to another one (`GET /pages/{id}/backlinks`). */
export interface Backlink {
  id: string
  title: string
  icon: string | null
  kind: string
  spaceKey: string
  spaceName: string
  /** how it links */
  via: 'link' | 'include' | 'property'
}

/** `GET /spaces/{key}/link-report` */
export interface LinkReport {
  brokenLinks: {
    pageId: string
    pageTitle: string
    blockId: string | null
    label: string
    via: 'link' | 'include' | 'property'
    problem: 'trashed' | 'deleted' | 'missing' | 'notImported'
    missingSpace: string | null
    missingTitle: string | null
    missingSource: string | null
  }[]
  orphans: { id: string; title: string; icon: string | null; parentId: string | null; parentTitle: string | null; updatedAt: string }[]
  truncated: boolean
  pending: number
}

export interface Page {
  id: string
  spaceId: string
  /** readable address `/wiki/<spaceKey>/<slug>` (null: only the permanent `/pages/<id>`) */
  slug?: string | null
  spaceKey: string
  spaceName: string
  parentId: string | null
  kind: string
  title: string
  icon: string | null
  cover?: string | null
  content: PmNode
  version: number
  status: string
  pageStatus: string | null
  /** the template new pages below this one start from */
  childTemplate?: string | null
  /** set when other people changed the page since the reader's previous visit */
  changedSince?: { version: number; at: string; versions: number; editors: string[]; blocks?: string[] } | null
  fullWidth: boolean
  createdByName: string | null
  createdAt: string
  updatedBy?: string | null
  updatedByName: string | null
  updatedAt: string
  /** responsible person: the owner if set, else the creator */
  ownerId?: string | null
  breadcrumbs: { id: string; title: string }[]
  labels: string[]
  /** typed key/value fields shown above the body */
  properties?: PageProperty[]
  likes?: { count: number; liked: boolean; names: string[] }
  access: PageAccess
  restricted: boolean
  favorite: boolean
  watching: boolean
  /** also watching the pages below */
  watchingChildren?: boolean
  /** watched through a page above, with the pages below */
  watchedThrough?: { id: string; title: string } | null
  views: number
  viewers: number
  commentCount: number
  /** an unpublished draft: only its creator sees it */
  draft?: boolean
}

export type PropertyType = 'text' | 'number' | 'date' | 'user' | 'select' | 'url' | 'page'

export interface PageProperty {
  key: string
  type: PropertyType
  /** text, select, url and date strings, numbers, user and page ids; null = empty */
  value: string | number | null
  /** the person's name or the linked page's title (null when gone or not visible) */
  label: string | null
}

export interface Comment {
  id: string
  pageId: string
  parentId: string | null
  authorId: string | null
  authorName: string | null
  content: PmNode
  inline: { text?: string } | null
  resolvedAt: string | null
  resolvedByName: string | null
  createdAt: string
  updatedAt: string
  likeCount?: number
  liked?: boolean
}

export interface Attachment {
  id: string
  pageId: string
  filename: string
  mediaType: string
  size: number
  version: number
  createdByName: string | null
  createdAt: string
  /** labels on the file (its page lists them; the label page lists the file) */
  labels?: string[]
}

export interface Version {
  version: number
  title: string
  createdBy?: string | null
  createdByName: string | null
  createdAt: string
  message: string | null
  /** watchers weren't notified about it */
  minor?: boolean
}

export interface SearchHit {
  pageId: string
  title: string
  snippet: string
  spaceKey: string
  spaceName: string
}

export interface PageListItem {
  id: string
  title: string
  spaceKey: string
  spaceName: string
  updatedAt: string
  updatedByName: string | null
  at: string | null
  /** drafts: when it publishes itself */
  publishAt?: string | null
}

export interface Job {
  id: string
  kind: string
  status: 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled'
  progress: { phase?: string; done?: number; total?: number; space?: string }
  result: any
  error: string | null
  createdAt: string
  startedAt?: string | null
  finishedAt: string | null
  /** a running job was asked to stop */
  cancelling?: boolean
  attempts?: number
  /** failed or cancelled and can run again */
  retryable?: boolean
}

// How dates are written: the workspace's format (Admin → Overview → Look) in the reader's time
// zone (profile, else the workspace's, else the browser's). Set once /me has loaded.
let dateFormat = 'auto'
let timeZone: string | undefined

export function setDateConventions(format?: string | null, tz?: string | null) {
  dateFormat = format || 'auto'
  timeZone = tz || undefined
  try {
    new Intl.DateTimeFormat(undefined, { timeZone })
  } catch {
    timeZone = undefined
  }
}

/** The time zone dates are shown in (undefined = the browser's). */
export function displayTimeZone(): string | undefined {
  return timeZone
}

/** A calendar day in the workspace's format. `zoned: false` for date-only values, which must
 *  not move a day when the reader's time zone is behind UTC. */
export function formatDay(d: Date, opts: { zoned?: boolean; omitYear?: boolean } = {}): string {
  const tz = opts.zoned === false ? 'UTC' : timeZone
  if (dateFormat === 'auto' || dateFormat === 'd mmm yyyy') {
    // the interface language writes the month (`5 Mar 2026`, `5. März 2026`)
    const tag = dateFormat === 'auto' ? undefined : uiLocale() === 'en' ? 'en-GB' : uiLocale()
    return d.toLocaleDateString(tag ?? uiLocale(), { timeZone: tz, year: opts.omitYear ? undefined : 'numeric', month: 'short', day: 'numeric' })
  }
  const parts = Object.fromEntries(new Intl.DateTimeFormat('en-US', { timeZone: tz, year: 'numeric', month: '2-digit', day: '2-digit' }).formatToParts(d).map((p) => [p.type, p.value]))
  return dateFormat.replace('yyyy', parts.year).replace('mm', parts.month).replace('dd', parts.day)
}

/** A date-only value (`YYYY-MM-DD`, or anything `Date` parses) as a calendar day. */
export function formatDateOnly(value: string, opts: { omitYear?: boolean } = {}): string {
  const d = /^\d{4}-\d{2}-\d{2}$/.test(value) ? new Date(`${value}T00:00:00Z`) : new Date(value)
  if (isNaN(d.getTime())) return value
  return formatDay(d, { zoned: !/^\d{4}-\d{2}-\d{2}$/.test(value), omitYear: opts.omitYear })
}

/** Day and time, e.g. for scheduled publishing. */
export function formatDateTime(iso: string | Date): string {
  const d = typeof iso === 'string' ? new Date(iso) : iso
  return `${formatDay(d)}, ${d.toLocaleTimeString(uiLocale(), { timeZone, hour: '2-digit', minute: '2-digit' })}`
}

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  const diff = (Date.now() - d.getTime()) / 1000
  // future dates (due dates, expiries) are always absolute
  if (diff < -60) return formatDay(d)
  const rel = new Intl.RelativeTimeFormat(uiLocale(), { numeric: 'auto', style: 'short' })
  if (diff < 60) return rel.format(0, 'second')
  if (diff < 3600) return rel.format(-Math.floor(diff / 60), 'minute')
  if (diff < 86400) return rel.format(-Math.floor(diff / 3600), 'hour')
  if (diff < 86400 * 7) return rel.format(-Math.floor(diff / 86400), 'day')
  return formatDay(d)
}

/** A space role as people read it. */
export function spaceRoleName(role: string): string {
  switch (role) {
    case 'viewer':
      return t('viewer [[space role]]')
    case 'commenter':
      return t('commenter [[space role]]')
    case 'editor':
      return t('editor [[space role]]')
    case 'admin':
      return t('admin [[space role]]')
    case 'none':
      return t('No access')
    default:
      return role
  }
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`
}

/** Stable color per user (cursors, avatars). Hex, because the collaboration caret plugin requires it. */
export function userColor(seed: string): string {
  let h = 0
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0
  const hue = h % 360
  const s = 0.7
  const rgb = (l: number) =>
    [0, 8, 4].map((n) => {
      const k = (n + hue / 30) % 12
      return Math.round((l - s * Math.min(l, 1 - l) * Math.max(-1, Math.min(k - 3, 9 - k, 1))) * 255)
    })
  // white initials sit on this color: darken bright hues (greens, yellows) to at least 4.5:1 (with margin)
  const luminance = (c: number[]) =>
    c.map((v) => (v / 255 <= 0.03928 ? v / 255 / 12.92 : ((v / 255 + 0.055) / 1.055) ** 2.4)).reduce((a, v, i) => a + v * [0.2126, 0.7152, 0.0722][i], 0)
  let l = 0.45
  while (l > 0.2 && 1.05 / (luminance(rgb(l)) + 0.05) < 4.6) l -= 0.01
  return `#${rgb(l).map((v) => v.toString(16).padStart(2, '0')).join('')}`
}

export function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((p) => p[0]!.toUpperCase())
    .join('')
}
