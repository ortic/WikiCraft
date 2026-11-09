import { createContext, useContext, useEffect, useState, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { api, initials, userColor, type Me } from './api'

export function useMe() {
  return useQuery({ queryKey: ['me'], queryFn: () => api.get<Me>('/me'), staleTime: 60_000 })
}

export function Logo() {
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

export function Avatar({ name, id, size = 26 }: { name: string; id?: string; size?: number }) {
  return (
    <span className="avatar" style={{ background: userColor(id ?? name), width: size, height: size, fontSize: size * 0.42 }} title={name}>
      {initials(name)}
    </span>
  )
}

export function SpaceIcon({ space, size = 24 }: { space: { key: string; name: string; icon?: string | null }; size?: number }) {
  if (space.icon) return <span style={{ fontSize: size * 0.8, width: size, textAlign: 'center' }}>{space.icon}</span>
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
