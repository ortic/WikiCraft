import { useState } from 'react'
import { SymbolTile, parseSymbol } from '../icons'
import { useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowLeft, Eye, FilePlus2, Link2 } from 'lucide-react'
import { api } from '../api'
import { Modal, Spinner, useToast } from '../ui'

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
  // new pages start as private drafts unless the person said otherwise last time
  const [draft, setDraft] = useState(() => {
    try {
      return localStorage.getItem('wc-new-page-draft') !== 'false'
    } catch {
      return true
    }
  })
  const rememberDraft = (v: boolean) => {
    setDraft(v)
    try {
      localStorage.setItem('wc-new-page-draft', String(v))
    } catch {
      /* private mode */
    }
  }
  const templates = useQuery({ queryKey: ['templates', spaceKey], queryFn: () => api.get<Template[]>(`/templates?spaceKey=${encodeURIComponent(spaceKey)}`) })
  const space = useQuery({ queryKey: ['space', spaceKey], queryFn: () => api.get<{ defaultTemplate?: string | null }>(`/spaces/${spaceKey}`) })
  const def = space.data?.defaultTemplate ?? null
  const ordered = [...(templates.data ?? [])].sort((a, b) => Number(b.id === def) - Number(a.id === def))
  const create = async (templateId?: string) => {
    setBusy(true)
    try {
      const r = await api.post<{ id: string }>('/pages', { spaceKey, parentId: parentId ?? undefined, templateId, title: templateId ? undefined : 'Untitled', blank: !templateId, draft })
      qc.invalidateQueries({ queryKey: ['tree', spaceKey] })
      onClose()
      nav(`/pages/${r.id}?new=1`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not create page')
    } finally {
      setBusy(false)
    }
  }
  const createLink = async () => {
    const url = window.prompt('Address of the website (https://…)')?.trim()
    if (!url) return
    const title = window.prompt('Name in the page tree (leave empty to use the site name)')?.trim()
    setBusy(true)
    try {
      await api.post<{ id: string }>('/pages', { spaceKey, parentId: parentId ?? undefined, kind: 'link', url, title: title || undefined })
      qc.invalidateQueries({ queryKey: ['tree', spaceKey] })
      toast('Link added to the page tree')
      onClose()
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not add the link')
    } finally {
      setBusy(false)
    }
  }
  const [preview, setPreview] = useState<Template | null>(null)
  const previewed = useQuery({
    queryKey: ['template-preview', preview?.id],
    queryFn: () => api.get<{ title: string | null; html: string }>(`/templates/${encodeURIComponent(preview!.id)}/preview`),
    enabled: !!preview,
  })
  const card = (key: string, icon: React.ReactNode, name: string, desc: string | null, onClick: () => void, badge?: string, onPreview?: () => void) => (
    <div key={key} className="template-card">
      <button className="space-card" style={{ textAlign: 'left', cursor: 'pointer', alignItems: 'flex-start', width: '100%' }} disabled={busy} onClick={onClick}>
        <span style={{ width: 32, display: 'flex', justifyContent: 'center' }}>{icon}</span>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontWeight: 600 }}>
            {name} {badge && <span className="badge">{badge}</span>}
          </div>
          {desc && <div className="faint small">{desc}</div>}
        </div>
      </button>
      {onPreview && (
        <button type="button" className="icon-btn template-preview-btn" title={`Preview ${name}`} aria-label={`Preview ${name}`} onClick={onPreview}>
          <Eye size={15} />
        </button>
      )}
    </div>
  )
  if (preview) {
    return (
      <Modal
        title={`Template: ${preview.name}`}
        onClose={onClose}
        width={760}
        footer={
          <>
            <button className="btn" onClick={() => setPreview(null)}><ArrowLeft size={14} /> All templates</button>
            <button className="btn btn-primary" disabled={busy} onClick={() => create(preview.id)}>Use this template</button>
          </>
        }
      >
        {previewed.isLoading ? (
          <Spinner />
        ) : previewed.data ? (
          <div className="template-preview">
            {previewed.data.title && <h1>{previewed.data.title}</h1>}
            {/* rendered by the server from the template (escaped, no scripts) */}
            <article className="ProseMirror public-article" dangerouslySetInnerHTML={{ __html: previewed.data.html }} />
          </div>
        ) : (
          <div className="empty">The preview isn't available.</div>
        )}
      </Modal>
    )
  }
  return (
    <Modal title="Create a page" onClose={onClose} width={760}>
      <label className="row small draft-option">
        <input type="checkbox" checked={draft} onChange={(e) => rememberDraft(e.target.checked)} />
        Start as a private draft: only you see it until you publish it
      </label>
      <div className="grid-cards">
        {card('blank', <FilePlus2 size={22} />, 'Blank page', 'Start from scratch', () => create())}
        {card('link', <Link2 size={22} />, 'Link to a website', 'A page-tree item that opens an external page', () => void createLink())}
        {ordered.map((t) => card(t.id, <SymbolTile value={parseSymbol(t.icon) ? t.icon : 'document:slate'} size={30} />, t.name, t.description, () => create(t.id), t.id === def ? 'space default' : t.builtin ? undefined : t.spaceKey ? 'space' : 'workspace', () => setPreview(t)))}
      </div>
    </Modal>
  )
}
