import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { FilePlus2 } from 'lucide-react'
import { api, type Space } from '../api'
import { Modal, useFeatures } from '../ui'
import { LineIcon } from '../icons'

interface SpaceTemplate {
  id: string
  name: string
  icon: string
  description: string
  pages: string[]
}

export default function CreateSpaceDialog({ onClose }: { onClose: () => void }) {
  const [name, setName] = useState('')
  const [key, setKey] = useState('')
  const [keyTouched, setKeyTouched] = useState(false)
  const [description, setDescription] = useState('')
  const features = useFeatures()
  const [access, setAccess] = useState(features.newSpaceRole ?? 'editor')
  const [template, setTemplate] = useState('')
  const templates = useQuery({ queryKey: ['space-templates'], queryFn: () => api.get<SpaceTemplate[]>('/space-templates'), staleTime: Infinity })
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const qc = useQueryClient()
  const nav = useNavigate()
  const derived = keyTouched ? key : name.replace(/[^A-Za-z0-9]/g, '').slice(0, 10).toUpperCase()
  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      const s = await api.post<Space>('/spaces', { name, key: derived, description, defaultRole: access, template: template || undefined })
      await qc.invalidateQueries({ queryKey: ['spaces'] })
      onClose()
      nav(`/spaces/${s.key}`)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed')
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal
      title="Create a space"
      width={620}
      onClose={onClose}
      footer={
        <>
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!name.trim() || !derived || busy} onClick={submit}>
            Create space
          </button>
        </>
      }
    >
      <div className="field">
        <label>Start with</label>
        <div className="space-templates">
          <button type="button" className={`space-template ${template === '' ? 'selected' : ''}`} onClick={() => setTemplate('')}>
            <FilePlus2 size={18} />
            <b>Blank space</b>
            <span className="faint small">Just a home page.</span>
          </button>
          {templates.data?.map((t) => (
            <button type="button" key={t.id} className={`space-template ${template === t.id ? 'selected' : ''}`} onClick={() => setTemplate(t.id)} title={t.pages.join(', ')}>
              <LineIcon group="symbols" name={t.icon} size={18} />
              <b>{t.name}</b>
              <span className="faint small">{t.description}</span>
            </button>
          ))}
        </div>
      </div>
      <div className="field">
        <label>Name</label>
        <input className="input" autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder="e.g. Engineering" />
      </div>
      <div className="field">
        <label>Key</label>
        <input className="input" value={derived} onChange={(e) => { setKeyTouched(true); setKey(e.target.value.toUpperCase()) }} placeholder="ENG" />
        <span className="faint small">Short identifier used in links. Letters, digits, _ and -.</span>
      </div>
      <div className="field">
        <label>Description</label>
        <textarea className="textarea" value={description} onChange={(e) => setDescription(e.target.value)} />
      </div>
      <div className="field">
        <label>Access for all members</label>
        <select className="select" value={access} onChange={(e) => setAccess(e.target.value)}>
          <option value="editor">Everyone can edit</option>
          <option value="commenter">Everyone can view and comment</option>
          <option value="viewer">Everyone can view</option>
          <option value="none">Private — only people I add</option>
        </select>
      </div>
      {error && <div className="error-text">{error}</div>}
    </Modal>
  )
}
