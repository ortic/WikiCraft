import { useRef, useState } from 'react'
import { ImagePlus, Link2, Smile, Trash2, Upload } from 'lucide-react'
import { api, type Attachment, type Page } from '../api'
import { SymbolPicker, SymbolTile, parseSymbol } from '../icons'
import { useToast } from '../ui'

export const GRADIENTS: Record<string, string> = {
  sunrise: 'linear-gradient(120deg, #f97316 0%, #ec4899 100%)',
  ocean: 'linear-gradient(120deg, #2563eb 0%, #06b6d4 100%)',
  forest: 'linear-gradient(120deg, #15803d 0%, #0d9488 100%)',
  dusk: 'linear-gradient(120deg, #7c3aed 0%, #312e81 100%)',
  slate: 'linear-gradient(120deg, #334155 0%, #64748b 100%)',
  mint: 'linear-gradient(120deg, #10b981 0%, #a3e635 100%)',
}

export function coverStyle(cover: string): React.CSSProperties {
  if (cover.startsWith('gradient:')) return { background: GRADIENTS[cover.slice(9)] ?? GRADIENTS.slate }
  return { backgroundImage: `url("${cover.replace(/"/g, '')}")`, backgroundSize: 'cover', backgroundPosition: 'center' }
}

/** Cover banner + page icon, with editing controls for people who can edit the page. */
export function PageDecor({ page, canEdit, onChange }: { page: Page; canEdit: boolean; onChange: () => void }) {
  const toast = useToast()
  const [menu, setMenu] = useState<'icon' | 'cover' | null>(null)
  const file = useRef<HTMLInputElement>(null)
  const save = async (body: Record<string, string>) => {
    try {
      await api.patch(`/pages/${page.id}`, body)
      onChange()
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Failed')
    }
  }
  const upload = async (f: File) => {
    const form = new FormData()
    form.append('file', f)
    const rows = await api.upload<Attachment[]>(`/pages/${page.id}/attachments`, form)
    if (rows[0]) await save({ cover: `/api/attachments/${rows[0].id}/content` })
    setMenu(null)
  }
  const icon = parseSymbol(page.icon) ? page.icon : null
  return (
    <>
      {page.cover && (
        <div className="page-cover" style={coverStyle(page.cover)}>
          {canEdit && (
            <div className="cover-actions">
              <button className="btn small" onClick={() => setMenu(menu === 'cover' ? null : 'cover')}>Change cover</button>
              <button className="btn small" onClick={() => save({ cover: '' })}><Trash2 size={13} /></button>
            </div>
          )}
        </div>
      )}
      <div className={`page-decor${page.cover ? ' with-cover' : ''}`}>
        {icon && (
          <button className="page-icon-btn" disabled={!canEdit} onClick={() => setMenu(menu === 'icon' ? null : 'icon')} title={canEdit ? 'Change icon' : undefined}>
            <SymbolTile value={icon} size={56} />
          </button>
        )}
        {canEdit && (
          <div className="decor-actions">
            {!icon && <button className="btn btn-ghost small" onClick={() => setMenu(menu === 'icon' ? null : 'icon')}><Smile size={14} /> Add icon</button>}
            {!page.cover && <button className="btn btn-ghost small" onClick={() => setMenu(menu === 'cover' ? null : 'cover')}><ImagePlus size={14} /> Add cover</button>}
          </div>
        )}
        {menu && <div className="decor-backdrop" onMouseDown={() => setMenu(null)} />}
        {menu === 'icon' && (
          <div className="decor-pop card">
            <SymbolPicker value={icon} onChange={(v) => save({ icon: v ?? '' })} />
            <div className="row" style={{ marginTop: 10 }}>
              <span className="spacer" />
              <button className="btn btn-ghost small" onClick={() => setMenu(null)}>Done</button>
            </div>
          </div>
        )}
        {menu === 'cover' && (
          <div className="decor-pop card">
            <div className="gradient-grid">
              {Object.entries(GRADIENTS).map(([k, g]) => (
                <button key={k} className="gradient-swatch" title={k} style={{ background: g }} onClick={() => { save({ cover: `gradient:${k}` }); setMenu(null) }} />
              ))}
            </div>
            <input ref={file} type="file" accept="image/png,image/jpeg,image/webp,image/gif" hidden onChange={(e) => e.target.files?.[0] && upload(e.target.files[0])} />
            <div className="row" style={{ marginTop: 10 }}>
              <button className="btn small" onClick={() => file.current?.click()}><Upload size={13} /> Upload image</button>
              <button className="btn small" onClick={() => {
                const url = window.prompt('Image URL (https://…)')
                if (url) { save({ cover: url.trim() }); setMenu(null) }
              }}><Link2 size={13} /> Image link</button>
              <span className="spacer" />
              <button className="btn btn-ghost small" onClick={() => setMenu(null)}>Close</button>
            </div>
          </div>
        )}
      </div>
    </>
  )
}
