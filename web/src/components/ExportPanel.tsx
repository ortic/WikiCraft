import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Archive, CheckCircle2, Download, FileCode2, FileText, Globe, XCircle } from 'lucide-react'
import { api, formatBytes, type Job, type Space } from '../api'
import { Spinner, useToast } from '../ui'

type Format = 'html' | 'markdown' | 'backup'

const FORMATS: { id: Format; icon: typeof Globe; title: string; text: string; adminOnly?: boolean }[] = [
  { id: 'html', icon: Globe, title: 'Website (HTML)', text: 'A static, browsable copy with the page tree and attachments. Open index.html in any browser — good for archives and offline reading.' },
  { id: 'markdown', icon: FileText, title: 'Markdown', text: 'One .md file per page in a folder tree, with front matter and attachments. Works with Git, Obsidian and static site generators, and imports back into WikiCraft.' },
  { id: 'backup', icon: Archive, title: 'Full backup', text: 'Everything needed to rebuild the space: pages, tree, labels, comments, attachments and restrictions. Restore it under Admin → Import.', adminOnly: true },
]

/** Space settings → Export: runs an export job and offers the archive for download. */
export default function ExportPanel({ space }: { space: Space }) {
  const toast = useToast()
  const [format, setFormat] = useState<Format>('html')
  const [jobId, setJobId] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const job = useQuery({
    queryKey: ['job', jobId],
    queryFn: () => api.get<Job>(`/jobs/${jobId}`),
    enabled: !!jobId,
    refetchInterval: (q) => (q.state.data && ['succeeded', 'failed'].includes(q.state.data.status) ? false : 1000),
  })
  const start = async () => {
    setBusy(true)
    try {
      const r = await api.post<{ jobId: string }>(`/spaces/${space.key}/export`, { format })
      setJobId(r.jobId)
    } catch (e) {
      toast(e instanceof Error ? e.message : 'Export failed')
    } finally {
      setBusy(false)
    }
  }
  const j = job.data
  const options = FORMATS.filter((f) => !f.adminOnly || space.role === 'admin')
  return (
    <div style={{ maxWidth: 720 }}>
      <p className="muted" style={{ marginTop: 0 }}>
        Exports contain the pages you can see{space.role === 'admin' ? '' : ' (restricted pages you have no access to are left out)'}. The download link is valid for 24 hours.
      </p>
      <div className="export-options" role="radiogroup" aria-label="Export format">
        {options.map((f) => (
          <label key={f.id} className={`export-option${format === f.id ? ' selected' : ''}`}>
            <input type="radio" name="format" value={f.id} checked={format === f.id} onChange={() => setFormat(f.id)} />
            <f.icon size={20} />
            <span>
              <b>{f.title}</b>
              <span className="muted small">{f.text}</span>
            </span>
          </label>
        ))}
      </div>
      <button className="btn btn-primary" style={{ marginTop: 14 }} disabled={busy || (j && !['succeeded', 'failed'].includes(j.status))} onClick={start}>
        {busy ? <Spinner /> : <FileCode2 size={15} />} Export {space.name}
      </button>
      {j && (
        <div className="card" style={{ marginTop: 18 }}>
          <div className="row">
            {j.status === 'succeeded' ? <CheckCircle2 color="var(--success)" size={18} /> : j.status === 'failed' ? <XCircle color="var(--danger)" size={18} /> : <Spinner />}
            <b>{j.status === 'succeeded' ? 'Export ready' : j.status === 'failed' ? 'Export failed' : 'Exporting…'}</b>
            <span className="faint small">
              {j.status !== 'succeeded' && j.progress?.phase}
              {j.status !== 'succeeded' && j.progress?.total ? ` · ${j.progress.done ?? 0}/${j.progress.total}` : ''}
            </span>
          </div>
          {j.error && <p className="error-text">{j.error}</p>}
          {j.status === 'succeeded' && j.result && (
            <div className="row" style={{ marginTop: 12, flexWrap: 'wrap' }}>
              <a className="btn" href={`/api/jobs/${j.id}/download`} download>
                <Download size={15} /> {j.result.filename}
              </a>
              <span className="faint small">
                {formatBytes(j.result.size)} · {j.result.pages} pages · {j.result.attachments} attachments
              </span>
            </div>
          )}
        </div>
      )}
    </div>
  )
}
