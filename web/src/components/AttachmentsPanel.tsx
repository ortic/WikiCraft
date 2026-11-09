import { useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Paperclip, Upload, Trash2, X, FileText, Image as ImageIcon } from 'lucide-react'
import { api, formatBytes, formatDate, type Attachment, type Page } from '../api'
import { useToast } from '../ui'

export default function AttachmentsPanel({ page, onClose }: { page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const input = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const list = useQuery({ queryKey: ['attachments', page.id], queryFn: () => api.get<Attachment[]>(`/pages/${page.id}/attachments`) })
  const upload = async (files: FileList | null) => {
    if (!files?.length) return
    setBusy(true)
    try {
      const form = new FormData()
      Array.from(files).forEach((f) => form.append('file', f, f.name))
      await api.upload(`/pages/${page.id}/attachments`, form)
      qc.invalidateQueries({ queryKey: ['attachments', page.id] })
      toast(`${files.length} file(s) uploaded`)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Upload failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <aside className="side-panel" onDragOver={(e) => e.preventDefault()} onDrop={(e) => { e.preventDefault(); if (page.access.edit) upload(e.dataTransfer.files) }}>
      <h3>
        <Paperclip size={16} /> Attachments <span className="spacer" />
        <button className="icon-btn" onClick={onClose}>
          <X size={16} />
        </button>
      </h3>
      {page.access.edit && (
        <>
          <input ref={input} type="file" multiple hidden onChange={(e) => upload(e.target.files)} />
          <button className="btn" style={{ width: '100%', justifyContent: 'center', marginBottom: 12 }} disabled={busy} onClick={() => input.current?.click()}>
            <Upload size={15} /> {busy ? 'Uploading…' : 'Upload files (or drop here)'}
          </button>
        </>
      )}
      {list.data?.map((a) => (
        <div key={a.id} className="comment row" style={{ gap: 10 }}>
          {a.mediaType.startsWith('image/') ? <ImageIcon size={18} className="faint" /> : <FileText size={18} className="faint" />}
          <div style={{ flex: 1, minWidth: 0 }}>
            <a href={`/api/attachments/${a.id}/content`} target="_blank" rel="noreferrer" style={{ fontWeight: 500, wordBreak: 'break-all' }}>
              {a.filename}
            </a>
            <div className="faint small">
              {formatBytes(a.size)} · v{a.version} · {formatDate(a.createdAt)}
            </div>
          </div>
          {page.access.edit && (
            <button
              className="icon-btn"
              title="Delete"
              onClick={async () => {
                if (!confirm(`Delete ${a.filename}?`)) return
                await api.del(`/attachments/${a.id}`)
                qc.invalidateQueries({ queryKey: ['attachments', page.id] })
              }}
            >
              <Trash2 size={14} />
            </button>
          )}
        </div>
      ))}
      {list.data?.length === 0 && <div className="empty">No attachments.</div>}
    </aside>
  )
}
