import { createContext, useContext, useEffect, useState, type ReactNode } from 'react'
import { SymbolTile, parseSymbol } from './icons'
import { useQuery } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { api, initials, userColor, type Me } from './api'

export function useMe() {
  return useQuery({ queryKey: ['me'], queryFn: () => api.get<Me>('/me'), staleTime: 60_000 })
}

export type Features = { publicLinks: boolean; exports: boolean; ai: boolean; personalSpaces: boolean; viewerList?: boolean; drawioUrl?: string | null }

/** Workspace switches as they apply to the signed-in person (Admin → Overview). Undefined while
 *  loading or when signed out: callers treat that as "on" (the server enforces them anyway). */
export function useFeatures(): Partial<Features> {
  const q = useQuery({ queryKey: ['features'], queryFn: () => api.get<Features>('/features'), staleTime: 300_000, retry: false })
  return q.data ?? {}
}

export function Logo() {
  // a workspace logo (Admin → Overview) replaces the WikiCraft mark, next to the workspace name
  const ws = useMe().data?.workspace
  if (ws?.logoVersion) {
    return (
      <span className="logo">
        <img className="logo-img" src={`/api/workspace/logo?v=${encodeURIComponent(ws.logoVersion)}`} alt="" />
        {ws.name}
      </span>
    )
  }
  return (
    <span className="logo">
      <svg viewBox="0 0 32 32" aria-hidden>
        <rect width="32" height="32" rx="7" fill="#4f46e5" />
        <path d="M7 10l3.5 12L14 13l2 0 3.5 9L23 10" fill="none" stroke="#fff" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
      WikiCraft
    </span>
  )
}

/** Profile pictures by person id (signed-in people only; guests and visitors see initials). */
export function useAvatarUrls() {
  const me = useMe()
  return useQuery({
    queryKey: ['avatar-urls'],
    queryFn: () => api.get<Record<string, string>>('/people/avatars'),
    enabled: !!me.data?.user,
    staleTime: 5 * 60_000,
  })
}

export function Avatar({ name, id, size = 26 }: { name: string; id?: string; size?: number }) {
  const urls = useAvatarUrls()
  const [broken, setBroken] = useState(false)
  const src = id && !broken ? urls.data?.[id] : undefined
  if (src) {
    return <img className="avatar avatar-img" src={src} alt="" title={name} width={size} height={size} style={{ width: size, height: size }} onError={() => setBroken(true)} />
  }
  return (
    <span className="avatar" style={{ background: userColor(id ?? name), width: size, height: size, fontSize: size * 0.42 }} title={name}>
      {initials(name)}
    </span>
  )
}

export function SpaceIcon({ space, size = 24 }: { space: { key: string; name: string; icon?: string | null }; size?: number }) {
  // icons are symbols from our set; anything else (e.g. legacy emoji) falls back to the letter
  if (parseSymbol(space.icon)) return <SymbolTile value={space.icon} size={size} />
  return (
    <span className="space-icon" style={{ background: userColor(space.key), width: size, height: size }}>
      {space.name.slice(0, 1).toUpperCase()}
    </span>
  )
}

export function Modal({ title, onClose, children, footer, width }: { title: ReactNode; onClose: () => void; children: ReactNode; footer?: ReactNode; width?: number }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={width ? { width: `min(${width}px, calc(100vw - 32px))` } : undefined} role="dialog" aria-modal>
        <div className="modal-header">
          {title}
          <span className="spacer" />
          <button className="icon-btn" onClick={onClose} aria-label="Close">
            <X size={16} />
          </button>
        </div>
        <div className="modal-body">{children}</div>
        {footer && <div className="modal-footer">{footer}</div>}
      </div>
    </div>
  )
}

const ToastCtx = createContext<(msg: string) => void>(() => {})

export function ToastProvider({ children }: { children: ReactNode }) {
  const [msg, setMsg] = useState<string | null>(null)
  useEffect(() => {
    if (!msg) return
    const t = setTimeout(() => setMsg(null), 2600)
    return () => clearTimeout(t)
  }, [msg])
  return (
    <ToastCtx.Provider value={setMsg}>
      {children}
      {msg && <div className="toast">{msg}</div>}
    </ToastCtx.Provider>
  )
}

export const useToast = () => useContext(ToastCtx)

export function Spinner() {
  return <div className="spinner" />
}

export function ErrorBox({ error }: { error: unknown }) {
  const msg = error instanceof Error ? error.message : String(error)
  return <div className="card error-text">{msg}</div>
}
