import { t as tr } from '../i18n'
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
  /** `{{variables}}` asked for when a page is created from the template */
  variables?: string[]
  /** blueprints: the index page new pages go under (unless created below a page) */
  filedUnder?: string | null
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
  // what a new page here starts from: a template set on a page above, or the space's default
  const fallback = useQuery({
    queryKey: ['default-template', spaceKey, parentId ?? null],
    queryFn: () =>
      api.get<{ templateId: string | null; from: 'space' | { pageId: string; title: string } | null }>(
        `/spaces/${spaceKey}/default-template${parentId ? `?parentId=${parentId}` : ''}`,
      ),
  })
  const def = fallback.data?.templateId ?? null
  const defLabel = fallback.data?.from && fallback.data.from !== 'space' ? tr('default below “{title}”', { title: fallback.data.from.title }) : 'space default'
  const ordered = [...(templates.data ?? [])].sort((a, b) => Number(b.id === def) - Number(a.id === def))
  // a template with variables asks for their values first
  const [filling, setFilling] = useState<Template | null>(null)
  const [answers, setAnswers] = useState<Record<string, string>>({})
  const choose = (t: Template) => {
    if (t.variables?.length) {
      setAnswers({})
      setFilling(t)
    } else {
      void create(t.id)
    }
  }
  const create = async (templateId?: string, variables?: Record<string, string>) => {
    setBusy(true)
    try {
      const r = await api.post<{ id: string }>('/pages', { spaceKey, parentId: parentId ?? undefined, templateId, title: templateId ? undefined : tr('Untitled'), blank: !templateId, draft, variables })
      qc.invalidateQueries({ queryKey: ['tree', spaceKey] })
      onClose()
      nav(`/pages/${r.id}?new=1`)
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not create page'))
    } finally {
      setBusy(false)
    }
  }
  const createLink = async () => {
    const url = window.prompt(tr('Address of the website (https://…)'))?.trim()
    if (!url) return
    const title = window.prompt(tr('Name in the page tree (leave empty to use the site name)'))?.trim()
    setBusy(true)
    try {
      await api.post<{ id: string }>('/pages', { spaceKey, parentId: parentId ?? undefined, kind: 'link', url, title: title || undefined })
      qc.invalidateQueries({ queryKey: ['tree', spaceKey] })
      toast(tr('Link added to the page tree'))
      onClose()
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not add the link'))
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
        <button type="button" className="icon-btn template-preview-btn" title={tr('Preview {name}', { name })} aria-label={tr('Preview {name}', { name })} onClick={onPreview}>
          <Eye size={15} />
        </button>
      )}
    </div>
  )
  if (filling) {
    const submit = (e: React.FormEvent) => {
      e.preventDefault()
      // only what was filled in: the rest stays a hint in the page
      const given = (filling.variables ?? []).map((v) => [v, (answers[v] ?? '').trim()]).filter(([, value]) => value)
      void create(filling.id, Object.fromEntries(given))
    }
    return (
      <Modal
        title={tr('New page: {name}', { name: filling.name })}
        onClose={onClose}
        footer={
          <>
            <button type="button" className="btn" onClick={() => setFilling(null)}><ArrowLeft size={14} />{' '}{tr('All templates')}</button>
            <button type="submit" form="template-variables" className="btn btn-primary" disabled={busy}>{busy ? <Spinner /> : null}{' '}{tr('Create page')}</button>
          </>
        }
      >
        <form id="template-variables" onSubmit={submit}>
          <p className="muted small" style={{ marginTop: 0 }}>{tr('Fill in what this template asks for. Anything you leave empty stays as a hint in the page.')}</p>
          {(filling.variables ?? []).map((v, i) => (
            <div key={v} className="field">
              <label htmlFor={`template-var-${i}`}>{v}</label>
              <input id={`template-var-${i}`} className="input" autoFocus={i === 0} maxLength={300} value={answers[v] ?? ''} onChange={(e) => setAnswers((a) => ({ ...a, [v]: e.target.value }))} />
            </div>
          ))}
        </form>
      </Modal>
    )
  }
  if (preview) {
    return (
      <Modal
        title={tr('Template: {name}', { name: preview.name })}
        onClose={onClose}
        width={760}
        footer={
          <>
            <button className="btn" onClick={() => setPreview(null)}><ArrowLeft size={14} />{' '}{tr('All templates')}</button>
            <button className="btn btn-primary" disabled={busy} onClick={() => choose(preview)}>{tr('Use this template')}</button>
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
          <div className="empty">{tr('The preview isn\'t available.')}</div>
        )}
      </Modal>
    )
  }
  return (
    <Modal title={tr('Create a page')} onClose={onClose} width={760}>
      <label className="row small draft-option">
        <input type="checkbox" checked={draft} onChange={(e) => rememberDraft(e.target.checked)} />{tr('Start as a private draft: only you see it until you publish it')}</label>
      <div className="grid-cards">
        {card('blank', <FilePlus2 size={22} />, tr('Blank page'), tr('Start from scratch'), () => create())}
        {card('link', <Link2 size={22} />, tr('Link to a website'), tr('A page-tree item that opens an external page'), () => void createLink())}
        {ordered.map((t) => card(t.id, <SymbolTile value={parseSymbol(t.icon) ? t.icon : 'document:slate'} size={30} />, t.name, !parentId && t.filedUnder ? tr('{value} · goes under “{filedUnder}”', { value: t.description ?? '', filedUnder: t.filedUnder }) : t.description, () => choose(t), t.id === def ? defLabel : t.builtin ? undefined : t.spaceKey ? 'space' : 'workspace', () => setPreview(t)))}
      </div>
    </Modal>
  )
}
