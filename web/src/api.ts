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

export const api = {
  get: <T>(p: string) => request<T>('GET', p),
  post: <T>(p: string, b?: unknown) => request<T>('POST', p, b ?? {}),
  put: <T>(p: string, b?: unknown) => request<T>('PUT', p, b ?? {}),
  patch: <T>(p: string, b?: unknown) => request<T>('PATCH', p, b ?? {}),
  del: <T>(p: string, b?: unknown) => request<T>('DELETE', p, b),
  upload: <T>(p: string, form: FormData) => request<T>('POST', p, form),
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
  workspace: { id: string; name: string } | null
}

export interface Space {
  id: string
  key: string
  name: string
  description: string | null
  icon: string | null
  homePageId: string | null
  defaultRole: Role
  anonymousAccess: boolean
  role: Role
  pageCount: number
  updatedAt: string
  archivedAt?: string | null
}

export interface TreeNode {
  id: string
  parentId: string | null
  title: string
  kind: string
  icon: string | null
  position: number
  restricted: boolean
}

export interface PageAccess {
  view: boolean
  comment: boolean
  edit: boolean
  admin: boolean
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
  content: PmNode
  version: number
  status: string
  pageStatus: string | null
  fullWidth: boolean
  createdByName: string | null
  createdAt: string
  updatedByName: string | null
  updatedAt: string
  breadcrumbs: { id: string; title: string }[]
  labels: string[]
  likes?: { count: number; liked: boolean; names: string[] }
  access: PageAccess
  restricted: boolean
  favorite: boolean
  watching: boolean
  views: number
  viewers: number
  commentCount: number
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
  createdByName: string | null
  createdAt: string
  message: string | null
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
}

export interface Job {
  id: string
  kind: string
  status: 'queued' | 'running' | 'succeeded' | 'failed'
  progress: { phase?: string; done?: number; total?: number; space?: string }
  result: any
  error: string | null
  createdAt: string
  finishedAt: string | null
}

export function formatDate(iso: string | null | undefined): string {
  if (!iso) return ''
  const d = new Date(iso)
  const diff = (Date.now() - d.getTime()) / 1000
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
  const l = 0.45
  const f = (n: number) => {
    const k = (n + hue / 30) % 12
    const c = l - s * Math.min(l, 1 - l) * Math.max(-1, Math.min(k - 3, 9 - k, 1))
    return Math.round(c * 255).toString(16).padStart(2, '0')
  }
  return `#${f(0)}${f(8)}${f(4)}`
}

export function initials(name: string): string {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((p) => p[0]!.toUpperCase())
    .join('')
}
