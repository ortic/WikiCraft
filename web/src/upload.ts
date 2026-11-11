// Uploads in parts (`/api/uploads`) for files too large for one request, such as multi-GB
// Confluence exports. A part that fails is sent again (with growing pauses); choosing the same
// file again after a reload carries on where the upload stopped.
import { ApiError, api } from './api'

export interface UploadState {
  id: string
  filename: string
  size: number
  partSize: number
  /** parts received so far: the next one to send has this number */
  parts: number
  received: number
  complete: boolean
}

const STORE = 'wikicraft.uploads'
const RETRIES = 8

function fileKey(f: File) {
  return `${f.name}:${f.size}:${f.lastModified}`
}

function remembered(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem(STORE) ?? '{}')
  } catch {
    return {}
  }
}

/** Notes (or forgets, with null) which upload holds this file, to resume it after a reload. */
export function rememberUpload(file: File, id: string | null) {
  try {
    const m = remembered()
    if (id) m[fileKey(file)] = id
    else delete m[fileKey(file)]
    localStorage.setItem(STORE, JSON.stringify(m))
  } catch {
    // private window: no resuming after a reload
  }
}

async function putPart(id: string, n: number, data: Blob, signal: AbortSignal): Promise<UploadState> {
  const res = await fetch(`/api/uploads/${id}/parts/${n}`, { method: 'PUT', headers: { 'x-wikicraft': '1' }, body: data, credentials: 'same-origin', signal })
  const text = await res.text()
  const body = text ? JSON.parse(text) : undefined
  if (!res.ok) throw new ApiError(res.status, body?.error ?? 'error', body?.message ?? res.statusText)
  return body as UploadState
}

const pause = (ms: number, signal: AbortSignal) =>
  new Promise<void>((resolve, reject) => {
    const t = setTimeout(resolve, ms)
    signal.addEventListener('abort', () => {
      clearTimeout(t)
      reject(new DOMException('aborted', 'AbortError'))
    })
  })

/** Uploads `file` in parts (continuing an earlier upload of the same file); `onProgress` gets the bytes received. */
export async function uploadInParts(file: File, onProgress: (received: number, size: number) => void, signal: AbortSignal): Promise<UploadState> {
  let state: UploadState | null = null
  const earlier = remembered()[fileKey(file)]
  if (earlier) state = await api.get<UploadState>(`/uploads/${earlier}`).catch(() => null)
  if (!state) {
    state = await api.post<UploadState>('/uploads', { filename: file.name, size: file.size })
    rememberUpload(file, state.id)
  }
  onProgress(state.received, state.size)
  let failures = 0
  while (!state.complete) {
    const n: number = state.parts
    const id: string = state.id
    const part = file.slice(n * state.partSize, Math.min(file.size, (n + 1) * state.partSize))
    try {
      state = await putPart(id, n, part, signal)
      failures = 0
    } catch (e) {
      if (signal.aborted) throw e
      // out of step (another tab, a lost answer): ask where it stands; refused parts don't get better by retrying
      if (e instanceof ApiError && e.status === 409) {
        state = await api.get<UploadState>(`/uploads/${id}`)
        continue
      }
      if (e instanceof ApiError && e.status < 500 && e.status !== 408 && e.status !== 429) throw e
      if (++failures > RETRIES) throw e
      await pause(Math.min(30_000, 500 * 2 ** failures), signal)
      continue
    }
    onProgress(state.received, state.size)
  }
  return state
}
