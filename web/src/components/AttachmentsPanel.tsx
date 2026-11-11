import { useRef, useState } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { Paperclip, Upload, Trash2, X, FileText, Image as ImageIcon, History, RotateCcw, Tag } from 'lucide-react'
import { Link } from 'react-router-dom'
import { api, can, formatBytes, formatDate, type Attachment, type Page } from '../api'
import { useToast } from '../ui'

export default function AttachmentsPanel({ page, onClose }: { page: Page; onClose: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const input = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const [history, setHistory] = useState<string | null>(null)
  const canAdd = page.access.edit && can(page.access, 'attachment.add')
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
    <aside className="side-panel" onDragOver={(e) => e.preventDefault()} onDrop={(e) => { e.preventDefault(); if (canAdd) upload(e.dataTransfer.files) }}>
      <h3>
        <Paperclip size={16} /> Attachments <span className="spacer" />
        <button className="icon-btn" onClick={onClose}>
          <X size={16} />
        </button>
      </h3>
      {canAdd && (
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
              {formatBytes(a.size)} ·{' '}
              {a.version > 1 ? (
                <button type="button" className="link-btn" aria-expanded={history === a.id} onClick={() => setHistory(history === a.id ? null : a.id)}>
                  <History size={11} /> v{a.version}
                </button>
              ) : (
                <>v{a.version}</>
              )}{' '}
              · {formatDate(a.createdAt)}
            </div>
            <FileLabels attachment={a} canEdit={page.access.edit} onSaved={() => qc.invalidateQueries({ queryKey: ['attachments', page.id] })} />
            {history === a.id && <VersionList attachmentId={a.id} current={a.version} canEdit={page.access.edit} onRestored={() => qc.invalidateQueries({ queryKey: ['attachments', page.id] })} />}
          </div>
          {page.access.edit && (can(page.access, 'attachment.delete') || can(page.access, 'delete.own')) && (
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

interface VersionRow {
  version: number
  size: number
  mediaType: string
  createdByName: string | null
  createdAt: string
}

/** Earlier versions of an attachment: download any, make one current again (a new version). */
function VersionList({ attachmentId, current, canEdit, onRestored }: { attachmentId: string; current: number; canEdit: boolean; onRestored: () => void }) {
  const qc = useQueryClient()
  const toast = useToast()
  const versions = useQuery({ queryKey: ['attachment-versions', attachmentId, current], queryFn: () => api.get<VersionRow[]>(`/attachments/${attachmentId}/versions`) })
  return (
    <ul className="attachment-versions">
      {versions.data?.map((v) => (
        <li key={v.version} className="row small">
          <a href={v.version === current ? `/api/attachments/${attachmentId}/content` : `/api/attachments/${attachmentId}/versions/${v.version}/content`} target="_blank" rel="noreferrer">
            v{v.version}
          </a>
          <span className="faint">
            {formatBytes(v.size)} · {formatDate(v.createdAt)}
            {v.createdByName ? ` · ${v.createdByName}` : ''}
          </span>
          <span className="spacer" />
          {v.version === current ? (
            <span className="badge">current</span>
          ) : (
            canEdit && (
              <button
                type="button"
                className="link-btn"
                title="Make this version the current one (as a new version)"
                onClick={async () => {
                  try {
                    const r = await api.post<{ version: number }>(`/attachments/${attachmentId}/versions/${v.version}/restore`)
                    qc.invalidateQueries({ queryKey: ['attachment-versions', attachmentId] })
                    onRestored()
                    toast(`v${v.version} is current again (as v${r.version})`)
                  } catch (e) {
                    toast(e instanceof Error ? e.message : 'Could not restore it')
                  }
                }}
              >
                <RotateCcw size={11} /> Make current
              </button>
            )
          )}
        </li>
      ))}
    </ul>
  )
}

/** A file's labels as links to the label pages; editors change them inline (comma-separated). */
function FileLabels({ attachment, canEdit, onSaved }: { attachment: Attachment; canEdit: boolean; onSaved: () => void }) {
  const toast = useToast()
  const [editing, setEditing] = useState(false)
  const [value, setValue] = useState('')
  const labels = attachment.labels ?? []
  const save = async () => {
    try {
      await api.put(`/attachments/${attachment.id}/labels`, { labels: value.split(',').map((l) => l.trim()).filter(Boolean) })
      setEditing(false)
      onSaved()
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Could not save the labels')
    }
  }
  if (editing)
    return (
      <div className="row" style={{ gap: 4, marginTop: 4 }}>
        <input
          className="input"
          style={{ height: 26, fontSize: 12 }}
          aria-label={`Labels of ${attachment.filename}`}
          placeholder="label, another"
          value={value}
          autoFocus
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void save()
            if (e.key === 'Escape') setEditing(false)
          }}
        />
        <button className="btn btn-sm" onClick={save}>Save</button>
      </div>
    )
  if (!labels.length && !canEdit) return null
  return (
    <div className="row" style={{ gap: 4, flexWrap: 'wrap', marginTop: 3 }}>
      {labels.map((l) => (
        <Link key={l} to={`/labels/${encodeURIComponent(l)}`} className="label-chip">{l}</Link>
      ))}
      {canEdit && (
        <button type="button" className="link-btn small" aria-label={`Edit the labels of ${attachment.filename}`} onClick={() => { setValue(labels.join(', ')); setEditing(true) }}>
          <Tag size={11} /> {labels.length ? 'Edit' : 'Add labels'}
        </button>
      )}
    </div>
  )
}
