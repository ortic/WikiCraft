// Templates people made from pages: saving a page as one (for its space or, for workspace
// admins, everyone), and the lists in Space settings and Admin → Templates to rename, describe,
// refresh or delete them.

import { t as tr } from '../i18n'
import { useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { LayoutTemplate, Pencil, Trash2 } from 'lucide-react'
import { api, type Page } from '../api'
import { Modal, useMe, useToast } from '../ui'

export interface TemplateItem {
  id: string
  name: string
  description: string | null
  builtin: boolean
  spaceKey: string | null
  canManage?: boolean
}

/** Page menu → Save as template. */
export function SaveTemplateDialog({ page, onClose }: { page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const me = useMe()
  const admin = me.data?.user?.role === 'owner' || me.data?.user?.role === 'admin'
  const [name, setName] = useState(page.title)
  const [description, setDescription] = useState('')
  const [everyone, setEveryone] = useState(false)
  const [busy, setBusy] = useState(false)
  const save = async () => {
    setBusy(true)
    try {
      await api.post('/templates', { name: name.trim(), description: description.trim() || undefined, pageId: page.id, spaceKey: everyone ? undefined : page.spaceKey })
      qc.invalidateQueries({ queryKey: ['templates'] })
      toast(everyone ? tr('Template “{name}” saved for every space', { name: name.trim() }) : tr('Template “{name}” saved for this space', { name: name.trim() }))
      onClose()
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save the template'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title={tr('Save as template')}
      onClose={onClose}
      width={480}
      footer={
        <>
          <button className="btn" onClick={onClose}>{tr('Cancel')}</button>
          <button className="btn btn-primary" disabled={busy || !name.trim()} onClick={save}>{tr('Save template')}</button>
        </>
      }
    >
      <p className="muted" style={{ marginTop: 0 }}>{tr('New pages can start from what this page holds now. Write')}{' '}<code>{tr('{{Name}}')}</code>{' '}{tr('in the page for something people fill in when they create one.')}</p>
      <div className="field">
        <label htmlFor="tpl-name">{tr('Name')}</label>
        <input id="tpl-name" className="input" maxLength={120} value={name} onChange={(e) => setName(e.target.value)} autoFocus />
      </div>
      <div className="field">
        <label htmlFor="tpl-desc">{tr('Description')}</label>
        <input id="tpl-desc" className="input" maxLength={500} placeholder={tr('When to use it')} value={description} onChange={(e) => setDescription(e.target.value)} />
      </div>
      {admin && (
        <fieldset className="field" style={{ border: 0, padding: 0, margin: 0 }}>
          <legend className="small" style={{ fontWeight: 500, marginBottom: 6 }}>{tr('Available in')}</legend>
          <label className="row" style={{ gap: 8 }}><input type="radio" name="tpl-scope" checked={!everyone} onChange={() => setEveryone(false)} />{' '}{tr('This space ({spaceKey})', { spaceKey: page.spaceKey })}</label>
          <label className="row" style={{ gap: 8, marginTop: 4 }}><input type="radio" name="tpl-scope" checked={everyone} onChange={() => setEveryone(true)} />{' '}{tr('Every space')}</label>
        </fieldset>
      )}
    </Modal>
  )
}

/** The templates of one space (`spaceKey`), or the workspace-wide ones (no `spaceKey`). */
export function TemplateList({ spaceKey }: { spaceKey?: string }) {
  const qc = useQueryClient()
  const toast = useToast()
  const list = useQuery({
    queryKey: ['templates', spaceKey ?? ''],
    queryFn: () => api.get<TemplateItem[]>(spaceKey ? `/templates?spaceKey=${encodeURIComponent(spaceKey)}` : '/templates'),
  })
  const [editing, setEditing] = useState<TemplateItem | null>(null)
  const mine = (list.data ?? []).filter((t) => !t.builtin && (spaceKey ? t.spaceKey?.toLowerCase() === spaceKey.toLowerCase() : !t.spaceKey))
  const remove = async (t: TemplateItem) => {
    if (!window.confirm(tr('Delete the template “{name}”? Pages made from it stay as they are.', { name: t.name }))) return
    try {
      await api.del(`/templates/${t.id}`)
      qc.invalidateQueries({ queryKey: ['templates'] })
      qc.invalidateQueries({ queryKey: ['space'] })
      toast(tr('Deleted “{name}”', { name: t.name }))
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not delete it'))
    }
  }
  if (list.isLoading) return null
  return (
    <>
      {mine.length === 0 ? (
        <p className="faint small" style={{ margin: 0 }}>{tr('None yet. Open a page and choose')}{' '}<i>{tr('Save as template')}</i>{' '}{tr('in its menu')}{spaceKey ? '' : tr(', then “Every space”')}.
        </p>
      ) : (
        <ul className="list-plain" style={{ margin: 0, padding: 0, listStyle: 'none' }}>
          {mine.map((t) => (
            <li key={t.id} className="row" style={{ gap: 8, padding: '6px 0', borderBottom: '1px solid var(--border)' }}>
              <LayoutTemplate size={15} style={{ flexShrink: 0 }} />
              <div style={{ minWidth: 0, flex: 1 }}>
                <div>{t.name}</div>
                {t.description && <div className="faint small">{t.description}</div>}
              </div>
              {t.canManage && (
                <>
                  <button className="icon-btn" title={tr('Rename or describe')} aria-label={tr('Edit {name}', { name: t.name })} onClick={() => setEditing(t)}><Pencil size={14} /></button>
                  <button className="icon-btn" title={tr('Delete')} aria-label={tr('Delete {name}', { name: t.name })} onClick={() => remove(t)}><Trash2 size={14} /></button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
      {editing && <EditTemplateDialog template={editing} onClose={() => setEditing(null)} />}
    </>
  )
}

function EditTemplateDialog({ template, onClose }: { template: TemplateItem; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const [name, setName] = useState(template.name)
  const [description, setDescription] = useState(template.description ?? '')
  const save = async () => {
    try {
      await api.patch(`/templates/${template.id}`, { name: name.trim(), description: description.trim() })
      qc.invalidateQueries({ queryKey: ['templates'] })
      toast(tr('Template saved'))
      onClose()
    } catch (e) {
      toast(e instanceof Error ? e.message : tr('Could not save'))
    }
  }
  return (
    <Modal
      title={tr('Edit template')}
      onClose={onClose}
      width={460}
      footer={
        <>
          <button className="btn" onClick={onClose}>{tr('Cancel')}</button>
          <button className="btn btn-primary" disabled={!name.trim()} onClick={save}>{tr('Save')}</button>
        </>
      }
    >
      <div className="field">
        <label htmlFor="tpl-edit-name">{tr('Name')}</label>
        <input id="tpl-edit-name" className="input" maxLength={120} value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="field" style={{ marginBottom: 0 }}>
        <label htmlFor="tpl-edit-desc">{tr('Description')}</label>
        <input id="tpl-edit-desc" className="input" maxLength={500} value={description} onChange={(e) => setDescription(e.target.value)} />
      </div>
    </Modal>
  )
}
