// Page status (Confluence's "Rough draft / In progress / Ready for review" lozenge): shown next to
// the page meta; editors click it (or "Set status…" in the page menu) to change it.

import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Check, X } from 'lucide-react'
import { api } from '../api'

export const PAGE_STATUSES: { value: string; color: string }[] = [
  { value: 'Rough draft', color: 'grey' },
  { value: 'In progress', color: 'blue' },
  { value: 'Ready for review', color: 'yellow' },
]

const colorOf = (s: string) => PAGE_STATUSES.find((x) => x.value.toLowerCase() === s.toLowerCase())?.color ?? 'purple'

interface Props {
  pageId: string
  status: string | null | undefined
  canEdit: boolean
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function PageStatus({ pageId, status, canEdit, open, onOpenChange }: Props) {
  const qc = useQueryClient()
  const [custom, setCustom] = useState('')
  const set = async (value: string) => {
    onOpenChange(false)
    await api.patch(`/pages/${pageId}`, { pageStatus: value })
    qc.invalidateQueries({ queryKey: ['page', pageId] })
  }
  if (!status && !open) return null
  return (
    <span className="page-status">
      {status ? (
        <button
          type="button"
          className="status-lozenge page-status-btn"
          data-color={colorOf(status)}
          disabled={!canEdit}
          title={canEdit ? 'Change the page status' : 'Page status'}
          aria-haspopup={canEdit ? 'menu' : undefined}
          aria-expanded={canEdit ? open : undefined}
          onClick={() => canEdit && onOpenChange(!open)}
        >
          {status}
        </button>
      ) : (
        <span className="faint small">Status:</span>
      )}
      {open && canEdit && (
        <div className="dropdown menu page-status-menu" role="menu" onMouseLeave={() => onOpenChange(false)}>
          <div className="menu-label">Page status</div>
          {PAGE_STATUSES.map((s) => (
            <div key={s.value} className="menu-item" role="menuitemradio" aria-checked={status === s.value} onClick={() => set(s.value)}>
              <span className="status-lozenge" data-color={s.color}>{s.value}</span>
              <span className="spacer" />
              {status === s.value && <Check size={14} />}
            </div>
          ))}
          <form
            className="row page-status-custom"
            onSubmit={(e) => {
              e.preventDefault()
              if (custom.trim()) set(custom.trim().slice(0, 40))
            }}
          >
            <input className="input" placeholder="Custom status" aria-label="Custom status" maxLength={40} value={custom} onChange={(e) => setCustom(e.target.value)} />
          </form>
          {status && (
            <>
              <div className="menu-sep" />
              <div className="menu-item" role="menuitem" onClick={() => set('')}>
                <X size={14} /> Remove status
              </div>
            </>
          )}
        </div>
      )}
    </span>
  )
}
