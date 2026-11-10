// Typed client for the WikiCraft REST API.

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
      throw new ApiError(400, 'bad_request', `${v.name} is larger than ${maxFileMb} MB, the largest file this wiki takes`)
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
  workspace: { id: string; name: string; logoVersion?: string | null } | null
  /** the workspace requires two-factor authentication and it isn't set up yet */
  mustSetUpTwoFactor?: boolean
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
}

export interface PmNode {
  type: string
  attrs?: Record<string, unknown>
  content?: PmNode[]
  text?: string
  marks?: { type: string; attrs?: Record<string, unknown> }[]
}

export interface Page {
  id: string
  spaceId: string
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

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  const diff = (Date.now() - d.getTime()) / 1000
  // future dates (due dates, expiries) are always absolute
  if (diff < -60) return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
  if (diff < 60) return 'just now'
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`
  if (diff < 86400) return `${Math.floor(diff / 3600)} h ago`
  if (diff < 86400 * 7) return `${Math.floor(diff / 86400)} d ago`
  return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
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
