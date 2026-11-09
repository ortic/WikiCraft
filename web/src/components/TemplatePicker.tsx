import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { FilePlus2 } from 'lucide-react'
import { api } from '../api'
import { Modal, useToast } from '../ui'

interface Template {
  id: string
  name: string
  description: string | null
  icon: string | null
  builtin: boolean
  spaceKey: string | null
}

/** "Create" dialog: blank page or one of the templates. */
export default function TemplatePicker({ spaceKey, parentId, onClose }: { spaceKey: string; parentId?: string | null; onClose: () => void }) {
  const nav = useNavigate()
  const qc = useQueryClient()
  const toast = useToast()
  const [busy, setBusy] = useState(false)
  const templates = useQuery({ queryKey: ['templates', spaceKey], queryFn: () => api.get<Template[]>(`/templates?spaceKey=${encodeURIComponent(spaceKey)}`) })
  const create = async (templateId?: string) => {
    setBusy(true)
    try {
      const r = await api.post<{ id: string }>('/pages', { spaceKey, parentId: parentId ?? undefined, templateId, title: templateId ? undefined : 'Untitled' })
      qc.invalidateQueries({ queryKey: ['tree', spaceKey] })
      onClose()
      nav(`/pages/${r.id}?new=1`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not create page')
    } finally {
      setBusy(false)
    }
  }
  const card = (key: string, icon: React.ReactNode, name: string, desc: string | null, onClick: () => void, badge?: string) => (
    <button key={key} className="space-card" style={{ textAlign: 'left', cursor: 'pointer', alignItems: 'flex-start' }} disabled={busy} onClick={onClick}>
      <span style={{ fontSize: 22, width: 30, textAlign: 'center' }}>{icon}</span>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontWeight: 600 }}>
          {name} {badge && <span className="badge">{badge}</span>}
        </div>
        {desc && <div className="faint small">{desc}</div>}
      </div>
    </button>
  )
  return (
    <Modal title="Create a page" onClose={onClose} width={760}>
      <div className="grid-cards">
        {card('blank', <FilePlus2 size={22} />, 'Blank page', 'Start from scratch', () => create())}
        {templates.data?.map((t) => card(t.id, t.icon ?? '📄', t.name, t.description, () => create(t.id), t.builtin ? undefined : t.spaceKey ? 'space' : 'workspace'))}
      </div>
    </Modal>
  )
}
